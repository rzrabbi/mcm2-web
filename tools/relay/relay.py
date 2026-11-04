"""WebSocket relay for Motocross Madness 2 network play (the "LAN party").

    python relay.py [--host 127.0.0.1] [--port 2108]

The game's own network code (net.rs in the engine) is a star: one host, and
every other player keeps one connection to the host. A browser cannot listen
for connections or broadcast, so this relay stands in for the LAN. A room is a
host plus up to seven guests, found by a short room code. The relay only
passes frames; it does not look inside them.

Control messages are JSON text frames, game frames are binary:

  client -> relay   {"t":"host","name":"..","public":true,"info":"..","parked":false}
  relay  -> host    {"t":"hosted","code":"K7QM2","max":8,"token":"..","entered":true,"ttl":300}
  client -> relay   {"t":"join","code":"K7QM2"}
  relay  -> guest   {"t":"joined","code":"K7QM2","id":3,"name":"..","info":".."}
  relay  -> host    {"t":"peer","id":3,"up":true}  (and up:false when they go)
  host   -> relay   {"t":"info","info":"..","public":true,"name":".."}
  relay  -> client  {"t":"error","reason":"no_room|not_open|full|busy|rate|bad_request"}

A room is open (joinable, listed) only while its host is in the game's lobby. A room made
on the start page is "parked": it exists, but has no players, cannot be joined and is not
listed, until the host says so:

  host   -> relay   {"t":"enter"}   the host is in the game's lobby: the room opens
  host   -> relay   {"t":"park"}    the host left the lobby: guests are sent out, the room
                                    closes to others but is kept
  host   -> relay   {"t":"close"}   delete the room
  client -> relay   {"t":"resume","code":"K7QM2","token":".."}   a host coming back (the page
                                    was reloaded, or the game was left) takes the room again

A parked room, or one whose host connection dropped, is deleted after PARK_SECONDS (300)
without its host entering the lobby. When an address is at its room limit, a new room replaces
one of its rooms that has no host (it is going to lapse anyway).

  guest -> relay -> host   payload       arrives at the host as: id byte + payload
  host  -> relay -> guest  id byte + payload   arrives at that guest as: payload
                           (id 255 goes to every guest)
  When the host leaves the lobby the guests are sent out (close code 4000).

Plain HTTP on the same port, for the start page's server browser:

  GET /mp/stats   {"public_rooms":n,"public_players":n,"rooms":n,"players":n}
  GET /mp/rooms   {"rooms":[{"code","name","players","max","info","age"}]}   (open public rooms)
  GET /mp/health  ok

`/mp` is optional in every path, so the relay works behind a proxy that keeps
or strips it. Set TRUST_PROXY=1 behind Nginx Proxy Manager so the client's
address comes from X-Forwarded-For (its last entry) instead of the proxy's.
Set ALLOWED_ORIGINS to a comma separated list of page origins that may connect
from another host (for a page on GitHub Pages: https://you.github.io); the relay's
own host and localhost are always allowed. Set PARK_SECONDS to change how long a
room nobody has entered is kept (default 300).

Run:  pip install -r requirements.txt && python relay.py --port 2108
Test: python -m unittest discover -s tests
"""
import argparse, asyncio, json, logging, os, re, secrets, time
from http import HTTPStatus
from urllib.parse import urlsplit

from websockets.asyncio.server import broadcast, serve
from websockets.exceptions import ConnectionClosed

log = logging.getLogger("relay")

MAX_PLAYERS = 8                 # the game's lobby holds eight names
ALL = 255                       # host -> relay: every guest
CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"   # no O/0/I/1
CODE_LEN = 5
MAX_FRAME = 64 * 1024
FIRST_MESSAGE_SECONDS = 10      # a connection must host or join by then
CLOSE_HOST_LEFT = 4000
CLOSE_FULL = 4001
CLOSE_REPLACED = 4002           # the host came back from another connection
CLOSE_EXPIRED = 4003            # a parked room nobody entered in time


class Limits:
    """Caps that keep an open relay from being used up. Tests shrink them."""
    rooms = 500
    connections = 2000
    connections_per_ip = 8
    rooms_per_ip = 3
    park_seconds = float(os.environ.get("PARK_SECONDS", "300"))
    bad_joins_per_minute = 12
    # Token buckets per connection: frames per second, bytes per second.
    host_rate = (4000, 8000)
    host_bytes = (1_000_000, 2_000_000)
    guest_rate = (300, 600)
    guest_bytes = (128_000, 256_000)


class Bucket:
    def __init__(self, per_second, burst):
        self.rate, self.burst, self.level, self.at = per_second, burst, burst, time.monotonic()

    def take(self, n=1):
        now = time.monotonic()
        self.level = min(self.burst, self.level + (now - self.at) * self.rate)
        self.at = now
        if self.level < n:
            return False
        self.level -= n
        return True


class Client:
    def __init__(self, ws, ip):
        self.ws, self.ip = ws, ip
        self.room = None
        self.id = None           # 0 for the host, 1..7 for guests
        self.frames = self.bytes = None

    def set_role(self, host):
        lim = Limits
        self.frames = Bucket(*(lim.host_rate if host else lim.guest_rate))
        self.bytes = Bucket(*(lim.host_bytes if host else lim.guest_bytes))

    def allow(self, size):
        return self.frames is None or (self.frames.take() and self.bytes.take(size))


class Room:
    def __init__(self, code, host, name, public, info, entered):
        self.code, self.host, self.name, self.public, self.info = code, host, name, public, info
        self.token = secrets.token_urlsafe(16)   # what the host shows to take the room back
        self.ip = host.ip
        self.guests = {}         # id -> Client
        self.created = time.time()
        self.entered = entered   # the host is in the game's lobby
        self.parked_since = None if entered else time.time()

    @property
    def open(self):
        """Joinable and listed: the host is connected and in the lobby."""
        return self.host is not None and self.entered

    def players(self):
        return 1 + len(self.guests)

    def free_id(self):
        return next((i for i in range(1, MAX_PLAYERS) if i not in self.guests), None)


class Relay:
    def __init__(self, trust_proxy=False, allowed_origins=()):
        self.rooms = {}
        self.clients = set()
        self.per_ip = {}
        self.bad_joins = {}      # ip -> [count, window start]
        self.trust_proxy = trust_proxy
        self.allowed_origins = {o.strip().rstrip("/").lower() for o in allowed_origins if o.strip()}

    # ---- helpers -----------------------------------------------------------------------

    def client_ip(self, ws):
        if self.trust_proxy:
            forwarded = ws.request.headers.get("X-Forwarded-For", "")
            if forwarded.strip():
                return forwarded.split(",")[-1].strip()
        peer = ws.remote_address
        return peer[0] if peer else "?"

    @staticmethod
    def text(value, limit):
        """A name or note from a client: printable characters only, shortened."""
        cleaned = "".join(ch for ch in str(value) if ch.isprintable()).strip()
        return cleaned[:limit]

    @staticmethod
    def normal_code(value):
        return re.sub(r"[^A-Z0-9]", "", str(value).upper())

    def new_code(self):
        for _ in range(50):
            code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
            if code not in self.rooms:
                return code
        return None

    async def say(self, client, **message):
        try:
            await client.ws.send(json.dumps(message, separators=(",", ":")))
        except ConnectionClosed:
            pass

    def origin_ok(self, request):
        origin = request.headers.get("Origin")
        if not origin:
            return True          # not a browser; a page cannot omit it
        origin = origin.rstrip("/").lower()
        host = (urlsplit(origin).hostname or "")
        if host in ("localhost", "127.0.0.1", "[::1]", "::1") or origin in self.allowed_origins:
            return True
        return (urlsplit(origin).netloc or "") == (request.headers.get("Host") or "").lower()

    # ---- stats for the server browser ----------------------------------------------

    def stats(self):
        # A room whose host is not in the lobby has no players and is not counted.
        open_rooms = [r for r in self.rooms.values() if r.open]
        public = [r for r in open_rooms if r.public]
        return {
            "public_rooms": len(public),
            "public_players": sum(r.players() for r in public),
            "rooms": len(open_rooms),
            "players": sum(r.players() for r in open_rooms),
        }

    def listing(self):
        now = time.time()
        rooms = sorted((r for r in self.rooms.values() if r.public and r.open), key=lambda r: r.created)
        return {"rooms": [
            {"code": r.code, "name": r.name, "players": r.players(), "max": MAX_PLAYERS,
             "info": r.info, "age": int(now - r.created)}
            for r in rooms
        ]}

    # ---- HTTP ----------------------------------------------------------------------------

    def http(self, connection, request):
        path = urlsplit(request.path).path
        if path.startswith("/mp/") or path == "/mp":
            path = path[3:] or "/"
        upgrade = request.headers.get("Upgrade", "").lower() == "websocket"
        if upgrade:
            if path not in ("/", "/ws"):
                return connection.respond(HTTPStatus.NOT_FOUND, "not found\n")
            if not self.origin_ok(request):
                return connection.respond(HTTPStatus.FORBIDDEN, "origin not allowed\n")
            return None          # carry on with the WebSocket handshake
        if path == "/stats":
            body = json.dumps(self.stats())
        elif path == "/rooms":
            body = json.dumps(self.listing())
        elif path == "/health":
            body = "ok"
        else:
            return connection.respond(HTTPStatus.NOT_FOUND, "not found\n")
        response = connection.respond(HTTPStatus.OK, body)
        del response.headers["Content-Type"]
        response.headers["Content-Type"] = "text/plain" if path == "/health" else "application/json"
        response.headers["Cache-Control"] = "no-store"
        origin = request.headers.get("Origin")
        if origin and self.origin_ok(request):
            # A page on another port or host (development) may read the lists.
            response.headers["Access-Control-Allow-Origin"] = origin
            response.headers["Vary"] = "Origin"
        return response

    # ---- WebSocket ------------------------------------------------------------------

    async def handle(self, ws):
        ip = self.client_ip(ws)
        client = Client(ws, ip)
        if len(self.clients) >= Limits.connections or self.per_ip.get(ip, 0) >= Limits.connections_per_ip:
            await ws.close(1013, "busy")
            return
        self.clients.add(client)
        self.per_ip[ip] = self.per_ip.get(ip, 0) + 1
        try:
            while True:
                try:
                    wait = FIRST_MESSAGE_SECONDS if client.room is None else None
                    message = await asyncio.wait_for(ws.recv(), wait)
                except asyncio.TimeoutError:
                    await ws.close(1008, "idle")
                    break
                except ConnectionClosed:
                    break
                size = len(message)
                if not client.allow(size):
                    await ws.close(1008, "rate")
                    break
                if isinstance(message, str):
                    await self.control(client, message)
                else:
                    self.relay(client, message)
        finally:
            self.clients.discard(client)
            self.per_ip[ip] -= 1
            if not self.per_ip[ip]:
                del self.per_ip[ip]
            await self.leave(client)

    async def control(self, client, raw):
        try:
            message = json.loads(raw)
            kind = message["t"]
        except (ValueError, KeyError, TypeError):
            return await self.say(client, t="error", reason="bad_request")
        if not isinstance(message, dict):
            return await self.say(client, t="error", reason="bad_request")
        if kind == "host" and client.room is None:
            await self.host(client, message)
        elif kind == "join" and client.room is None:
            await self.join(client, message)
        elif kind == "resume" and client.room is None:
            await self.resume(client, message)
        elif kind in ("enter", "park", "close") and client.room is not None and client.id == 0:
            await getattr(self, kind)(client)
        elif kind == "info" and client.room is not None and client.id == 0:
            room = client.room
            if "info" in message:
                room.info = self.text(message["info"], 80)
            if "name" in message:
                room.name = self.text(message["name"], 32) or room.name
            if "public" in message:
                room.public = bool(message["public"])
        elif kind == "ping":
            await self.say(client, t="pong")
        else:
            await self.say(client, t="error", reason="bad_request")

    async def host(self, client, message):
        if len(self.rooms) >= Limits.rooms:
            return await self.say(client, t="error", reason="busy")
        mine = [r for r in self.rooms.values() if r.ip == client.ip]
        if len(mine) >= Limits.rooms_per_ip:
            # Rooms this address made whose host is gone only wait for their time to run out:
            # the oldest give way to a new one (a player who left a room behind can make another).
            for gone in sorted((r for r in mine if r.host is None), key=lambda r: r.parked_since or 0):
                if len(mine) < Limits.rooms_per_ip:
                    break
                self.delete(gone)
                mine.remove(gone)
        if len(mine) >= Limits.rooms_per_ip:
            return await self.say(client, t="error", reason="busy")
        code = self.new_code()
        if code is None:
            return await self.say(client, t="error", reason="busy")
        room = Room(code, client, self.text(message.get("name", ""), 32) or "Room",
                    bool(message.get("public")), self.text(message.get("info", ""), 80),
                    entered=not message.get("parked"))
        self.rooms[code] = room
        client.room, client.id = room, 0
        client.set_role(True)
        log.info("room %s made (%s, %s)", code, "public" if room.public else "private",
                 "open" if room.entered else "parked")
        await self.say(client, **self.hosted_reply(room))

    def hosted_reply(self, room):
        return dict(t="hosted", code=room.code, max=MAX_PLAYERS, token=room.token,
                    entered=room.entered, name=room.name, public=room.public,
                    ttl=int(Limits.park_seconds))

    async def resume(self, client, message):
        """A host takes its room back with the token it was given."""
        if self.too_many_bad_joins(client.ip):
            return await self.say(client, t="error", reason="rate")
        room = self.rooms.get(self.normal_code(message.get("code", "")))
        token = str(message.get("token", ""))
        if room is None or not secrets.compare_digest(room.token, token):
            self.note_bad_join(client.ip)
            return await self.say(client, t="error", reason="no_room")
        old = room.host
        if old is not None and old is not client:
            # The old connection is stale (or a second tab): the newer one is the host.
            old.room = None
            self.kick_guests(room)
            asyncio.ensure_future(old.ws.close(CLOSE_REPLACED, "replaced"))
        room.host = client
        room.entered = False
        room.parked_since = time.time()
        client.room, client.id = room, 0
        client.set_role(True)
        log.info("room %s taken back by its host", room.code)
        await self.say(client, **self.hosted_reply(room))

    def kick_guests(self, room):
        for guest in list(room.guests.values()):
            guest.room = None
            asyncio.ensure_future(guest.ws.close(CLOSE_HOST_LEFT, "host_left"))
        room.guests.clear()

    async def enter(self, client):
        room = client.room
        room.entered, room.parked_since = True, None
        log.info("room %s open", room.code)
        await self.say(client, t="entered")

    async def park(self, client):
        """The host left the game's lobby: others are sent out, the room waits for the host."""
        room = client.room
        self.kick_guests(room)
        room.entered, room.parked_since = False, time.time()
        log.info("room %s parked", room.code)
        await self.say(client, t="parked")

    async def close(self, client):
        room = client.room
        self.delete(room)
        client.room = None

    def delete(self, room):
        self.rooms.pop(room.code, None)
        self.kick_guests(room)
        if room.host is not None:
            room.host.room = None
        log.info("room %s deleted", room.code)

    def sweep(self, now=None):
        """Deletes the rooms whose host has not been in the lobby for `park_seconds`."""
        now = time.time() if now is None else now
        for room in [r for r in self.rooms.values()
                     if not r.entered and now - r.parked_since > Limits.park_seconds]:
            host = room.host
            self.delete(room)
            if host is not None:
                asyncio.ensure_future(host.ws.close(CLOSE_EXPIRED, "expired"))

    async def sweep_loop(self):
        while True:
            await asyncio.sleep(10)
            self.sweep()

    def too_many_bad_joins(self, ip):
        now = time.time()
        count, start = self.bad_joins.get(ip, (0, now))
        if now - start > 60:
            count, start = 0, now
        self.bad_joins[ip] = (count, start)
        return count >= Limits.bad_joins_per_minute

    def note_bad_join(self, ip):
        count, start = self.bad_joins.get(ip, (0, time.time()))
        self.bad_joins[ip] = (count + 1, start)

    async def join(self, client, message):
        if self.too_many_bad_joins(client.ip):
            return await self.say(client, t="error", reason="rate")
        room = self.rooms.get(self.normal_code(message.get("code", "")))
        if room is None:
            self.note_bad_join(client.ip)
            return await self.say(client, t="error", reason="no_room")
        if not room.open:
            # Its host has not entered the lobby (or has left it): nobody can join yet.
            self.note_bad_join(client.ip)
            return await self.say(client, t="error", reason="not_open")
        gid = room.free_id()
        if gid is None:
            return await self.say(client, t="error", reason="full")
        room.guests[gid] = client
        client.room, client.id = room, gid
        client.set_role(False)
        await self.say(client, t="joined", code=room.code, id=gid, name=room.name, info=room.info)
        await self.say(room.host, t="peer", id=gid, up=True)

    def relay(self, client, data):
        room = client.room
        if room is None or not data or room.host is None:
            return
        if client.id == 0:       # host -> guest(s): first byte says which
            target, payload = data[0], data[1:]
            if target == ALL:
                broadcast([g.ws for g in room.guests.values()], payload)
            elif target in room.guests:
                broadcast([room.guests[target].ws], payload)
        else:                    # guest -> host, tagged with who sent it
            broadcast([room.host.ws], bytes([client.id]) + data)

    async def leave(self, client):
        room, client.room = client.room, None
        if room is None:
            return
        if client.id == 0:
            # The host's connection went (a reload, a closed tab): guests are sent out, but
            # the room is kept for it to come back to, until `park_seconds` pass.
            if room.host is client:
                room.host = None
                self.kick_guests(room)
                room.entered, room.parked_since = False, time.time()
                log.info("room %s lost its host", room.code)
        elif room.guests.get(client.id) is client:
            del room.guests[client.id]
            await self.say(room.host, t="peer", id=client.id, up=False)


async def start(host="127.0.0.1", port=2108, relay=None):
    relay = relay or Relay()
    server = await serve(relay.handle, host, port, process_request=relay.http, max_size=MAX_FRAME,
                         ping_interval=20, ping_timeout=20, compression=None)
    server.relay = relay
    return server


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default=os.environ.get("HOST", "127.0.0.1"))
    ap.add_argument("--port", type=int, default=2108)
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")
    relay = Relay(os.environ.get("TRUST_PROXY") == "1", os.environ.get("ALLOWED_ORIGINS", "").split(","))
    server = await start(args.host, args.port, relay)
    asyncio.ensure_future(relay.sweep_loop())
    log.info("relay on %s:%d", args.host, args.port)
    await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
