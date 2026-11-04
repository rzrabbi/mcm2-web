"""Run: python -m unittest discover -s tests   (needs the websockets package)"""
import asyncio, json, os, sys, unittest
from urllib.request import urlopen
from urllib.error import HTTPError

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import relay
from websockets.asyncio.client import connect
from websockets.exceptions import ConnectionClosed


class RelayTest(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.saved = {k: getattr(relay.Limits, k) for k in vars(relay.Limits) if not k.startswith("_")}
        self.server = await relay.start("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.open = []

    async def asyncTearDown(self):
        for ws in self.open:
            await ws.close()
        self.server.close()
        await self.server.wait_closed()
        for k, v in self.saved.items():
            setattr(relay.Limits, k, v)

    async def client(self, path="/mp/ws"):
        ws = await connect(f"ws://127.0.0.1:{self.port}{path}")
        self.open.append(ws)
        return ws

    async def ask(self, ws, **message):
        await ws.send(json.dumps(message))
        return json.loads(await asyncio.wait_for(ws.recv(), 3))

    async def hosted(self, **kw):
        host = await self.client()
        reply = await self.ask(host, t="host", **kw)
        self.assertEqual(reply["t"], "hosted")
        return host, reply["code"]

    def get(self, path):
        def fetch():
            with urlopen(f"http://127.0.0.1:{self.port}{path}", timeout=3) as r:
                return r.status, r.read().decode()
        return asyncio.to_thread(fetch)

    async def test_join_and_pass_frames(self):
        host, code = await self.hosted(name="Mike's", public=True)
        self.assertEqual(len(code), 5)
        guest = await self.client()
        reply = await self.ask(guest, t="join", code=code.lower())
        self.assertEqual((reply["t"], reply["id"], reply["name"]), ("joined", 1, "Mike's"))
        self.assertEqual(json.loads(await host.recv()), {"t": "peer", "id": 1, "up": True})
        await guest.send(b"hello")
        self.assertEqual(await host.recv(), bytes([1]) + b"hello")
        await host.send(bytes([1]) + b"back")
        self.assertEqual(await guest.recv(), b"back")
        guest2 = await self.client()
        self.assertEqual((await self.ask(guest2, t="join", code=code))["id"], 2)
        await host.recv()
        await host.send(bytes([relay.ALL]) + b"all")
        self.assertEqual(await guest.recv(), b"all")
        self.assertEqual(await guest2.recv(), b"all")

    async def test_guest_cannot_reach_other_guests(self):
        host, code = await self.hosted()
        a, b = await self.client(), await self.client()
        await self.ask(a, t="join", code=code)
        await self.ask(b, t="join", code=code)
        await a.send(b"x")
        await host.send(bytes([2]) + b"for b")
        self.assertEqual(await b.recv(), b"for b")   # a's frame never reached b first

    async def test_guest_leaves_and_host_leaves(self):
        host, code = await self.hosted()
        guest = await self.client()
        await self.ask(guest, t="join", code=code)
        await host.recv()
        await guest.close()
        self.assertEqual(json.loads(await host.recv()), {"t": "peer", "id": 1, "up": False})
        guest = await self.client()
        self.assertEqual((await self.ask(guest, t="join", code=code))["id"], 1)   # id reused
        await host.recv()
        await host.close()
        with self.assertRaises(ConnectionClosed) as caught:
            await asyncio.wait_for(guest.recv(), 3)
        self.assertEqual(caught.exception.rcvd.code, relay.CLOSE_HOST_LEFT)
        # The room is kept for its host to come back to, but nobody can join it meanwhile.
        reply = await self.ask(await self.client(), t="join", code=code)
        self.assertEqual(reply["reason"], "not_open")

    async def test_room_full(self):
        relay.Limits.connections_per_ip = 20
        host, code = await self.hosted()
        for i in range(1, relay.MAX_PLAYERS):
            self.assertEqual((await self.ask(await self.client(), t="join", code=code))["id"], i)
        self.assertEqual((await self.ask(await self.client(), t="join", code=code))["reason"], "full")

    async def test_browser_lists_only_public_rooms(self):
        await self.hosted(name="Open", public=True, info="Stadium")
        await self.hosted(name="Secret", public=False)
        status, body = await self.get("/mp/rooms")
        rooms = json.loads(body)["rooms"]
        self.assertEqual([(r["name"], r["info"], r["players"], r["max"]) for r in rooms], [("Open", "Stadium", 1, 8)])
        stats = json.loads((await self.get("/mp/stats"))[1])
        self.assertEqual(stats, {"public_rooms": 1, "public_players": 1, "rooms": 2, "players": 2})
        self.assertEqual((await self.get("/stats"))[0], 200)        # the prefix is optional
        self.assertEqual((await self.get("/mp/health"))[1], "ok")
        with self.assertRaises(HTTPError) as caught:
            await self.get("/mp/nothing")
        self.assertEqual(caught.exception.code, 404)

    async def test_host_updates_listing(self):
        host, code = await self.hosted(name="A", public=False)
        await host.send(json.dumps({"t": "info", "public": True, "info": "Track 4", "name": "B\x00"}))
        for _ in range(30):
            rooms = json.loads((await self.get("/mp/rooms"))[1])["rooms"]
            if rooms:
                break
            await asyncio.sleep(0.05)
        self.assertEqual((rooms[0]["name"], rooms[0]["info"]), ("B", "Track 4"))

    # ---- parked rooms: made on the start page, opened when the host is in the lobby ----

    async def parked(self, **kw):
        host = await self.client()
        reply = await self.ask(host, t="host", parked=True, **kw)
        self.assertEqual((reply["t"], reply["entered"]), ("hosted", False))
        self.assertTrue(reply["token"])
        return host, reply["code"], reply["token"]

    async def test_parked_room_is_hidden_and_closed_to_others(self):
        host, code, _ = await self.parked(name="Wait", public=True)
        self.assertEqual(json.loads((await self.get("/mp/rooms"))[1])["rooms"], [])
        stats = json.loads((await self.get("/mp/stats"))[1])
        self.assertEqual((stats["public_rooms"], stats["rooms"], stats["players"]), (0, 0, 0))
        self.assertEqual((await self.ask(await self.client(), t="join", code=code))["reason"], "not_open")

    async def test_entering_opens_and_parking_closes_it(self):
        host, code, _ = await self.parked(name="Room", public=True)
        self.assertEqual((await self.ask(host, t="enter"))["t"], "entered")
        rooms = json.loads((await self.get("/mp/rooms"))[1])["rooms"]
        self.assertEqual([(r["name"], r["players"]) for r in rooms], [("Room", 1)])
        guest = await self.client()
        self.assertEqual((await self.ask(guest, t="join", code=code))["t"], "joined")
        await host.recv()                                   # the guest came
        self.assertEqual((await self.ask(host, t="park"))["t"], "parked")
        with self.assertRaises(ConnectionClosed) as caught:  # the guest is sent out
            await asyncio.wait_for(guest.recv(), 3)
        self.assertEqual(caught.exception.rcvd.code, relay.CLOSE_HOST_LEFT)
        self.assertEqual(json.loads((await self.get("/mp/rooms"))[1])["rooms"], [])
        self.assertEqual((await self.ask(await self.client(), t="join", code=code))["reason"], "not_open")
        self.assertEqual((await self.ask(host, t="enter"))["t"], "entered")   # and back again
        self.assertEqual((await self.ask(await self.client(), t="join", code=code))["t"], "joined")

    async def test_host_comes_back_after_a_reload(self):
        host, code, token = await self.parked(name="Keep", public=True)
        await self.ask(host, t="enter")
        guest = await self.client()
        await self.ask(guest, t="join", code=code)
        await host.recv()
        await host.close()                                  # the page is reloaded
        with self.assertRaises(ConnectionClosed):
            await asyncio.wait_for(guest.recv(), 3)
        self.assertEqual((await self.ask(await self.client(), t="join", code=code))["reason"], "not_open")
        again = await self.client()
        reply = await self.ask(again, t="resume", code=code.lower(), token=token)
        self.assertEqual((reply["t"], reply["code"], reply["name"], reply["public"], reply["entered"]),
                         ("hosted", code, "Keep", True, False))
        self.assertEqual(json.loads((await self.get("/mp/rooms"))[1])["rooms"], [])   # still not entered
        await self.ask(again, t="enter")
        self.assertEqual(len(json.loads((await self.get("/mp/rooms"))[1])["rooms"]), 1)

    async def test_resume_needs_the_token(self):
        host, code, token = await self.parked()
        await host.close()
        other = await self.client()
        self.assertEqual((await self.ask(other, t="resume", code=code, token="wrong"))["reason"], "no_room")
        self.assertEqual((await self.ask(other, t="resume", code="ZZZZZ", token=token))["reason"], "no_room")
        self.assertEqual((await self.ask(other, t="resume", code=code, token=token))["t"], "hosted")

    async def test_resume_replaces_a_stale_host_connection(self):
        host, code, token = await self.parked()
        newer = await self.client()
        self.assertEqual((await self.ask(newer, t="resume", code=code, token=token))["t"], "hosted")
        with self.assertRaises(ConnectionClosed) as caught:
            await asyncio.wait_for(host.recv(), 3)
        self.assertEqual(caught.exception.rcvd.code, relay.CLOSE_REPLACED)

    async def test_parked_rooms_expire(self):
        relay.Limits.park_seconds = 60
        host, code, token = await self.parked()
        other, ocode, _ = await self.parked()
        await self.ask(other, t="enter")                    # this one is open: it stays
        self.server.relay.sweep(now=__import__("time").time() + 30)
        self.assertIn(code, self.server.relay.rooms)
        self.server.relay.sweep(now=__import__("time").time() + 120)
        self.assertNotIn(code, self.server.relay.rooms)
        self.assertIn(ocode, self.server.relay.rooms)
        with self.assertRaises(ConnectionClosed) as caught:
            await asyncio.wait_for(host.recv(), 3)
        self.assertEqual(caught.exception.rcvd.code, relay.CLOSE_EXPIRED)
        self.assertEqual((await self.ask(await self.client(), t="resume", code=code, token=token))["reason"], "no_room")

    async def test_a_lost_host_room_expires_too(self):
        relay.Limits.park_seconds = 60
        host, code = await self.hosted()
        await host.close()
        for _ in range(40):
            if self.server.relay.rooms[code].host is None:
                break
            await asyncio.sleep(0.05)
        self.server.relay.sweep(now=__import__("time").time() + 120)
        self.assertNotIn(code, self.server.relay.rooms)

    async def test_a_new_room_replaces_ones_whose_host_is_gone(self):
        relay.Limits.rooms_per_ip = 2
        relay.Limits.connections_per_ip = 20
        first, code1, _ = await self.parked()
        second, code2, _ = await self.parked()
        # Both hosts are live: no room for a third.
        self.assertEqual((await self.ask(await self.client(), t="host"))["reason"], "busy")
        await first.close()                                 # its host is gone (a reload, a tab closed)
        for _ in range(40):
            if self.server.relay.rooms[code1].host is None:
                break
            await asyncio.sleep(0.05)
        third = await self.client()
        self.assertEqual((await self.ask(third, t="host"))["t"], "hosted")
        self.assertNotIn(code1, self.server.relay.rooms)     # the abandoned room gave way
        self.assertIn(code2, self.server.relay.rooms)        # a live host's room did not

    async def test_close_deletes_the_room(self):
        host, code, _ = await self.parked()
        await host.send(json.dumps({"t": "close"}))
        for _ in range(40):
            if code not in self.server.relay.rooms:
                break
            await asyncio.sleep(0.05)
        self.assertNotIn(code, self.server.relay.rooms)

    async def test_rooms_made_in_the_game_open_at_once(self):
        host, code = await self.hosted(name="Now", public=True)
        self.assertEqual(len(json.loads((await self.get("/mp/rooms"))[1])["rooms"]), 1)

    async def test_bad_requests(self):
        ws = await self.client()
        await ws.send("not json")
        self.assertEqual(json.loads(await ws.recv())["reason"], "bad_request")
        self.assertEqual((await self.ask(ws, t="nope"))["reason"], "bad_request")
        await ws.send("[1]")
        self.assertEqual(json.loads(await ws.recv())["reason"], "bad_request")

    async def test_silent_connection_dropped(self):
        relay.FIRST_MESSAGE_SECONDS = 0.2
        ws = await self.client()
        with self.assertRaises(ConnectionClosed):
            await asyncio.wait_for(ws.recv(), 3)
        relay.FIRST_MESSAGE_SECONDS = 10

    async def test_limits(self):
        relay.Limits.rooms_per_ip = 1
        await self.hosted()
        self.assertEqual((await self.ask(await self.client(), t="host"))["reason"], "busy")
        relay.Limits.bad_joins_per_minute = 2
        ws = await self.client()
        self.assertEqual((await self.ask(ws, t="join", code="AAAAA"))["reason"], "no_room")
        ws2 = await self.client()
        await self.ask(ws2, t="join", code="AAAAA")
        self.assertEqual((await self.ask(await self.client(), t="join", code="AAAAA"))["reason"], "rate")
        relay.Limits.connections_per_ip = 1
        extra = await self.client()      # the per-ip count is already above 1
        with self.assertRaises(ConnectionClosed):
            await asyncio.wait_for(extra.recv(), 3)

    async def test_guest_flood_is_cut(self):
        relay.Limits.guest_rate = (1, 5)
        host, code = await self.hosted()
        guest = await self.client()
        await self.ask(guest, t="join", code=code)
        with self.assertRaises(ConnectionClosed):
            for _ in range(200):
                await guest.send(b"x")
                await asyncio.sleep(0)
            await asyncio.wait_for(guest.recv(), 3)

    async def test_foreign_origin_refused(self):
        with self.assertRaises(Exception):
            await connect(f"ws://127.0.0.1:{self.port}/mp/ws", origin="https://evil.example")
        ok = await connect(f"ws://127.0.0.1:{self.port}/mp/ws", origin=f"http://127.0.0.1:{self.port}")
        self.open.append(ok)


if __name__ == "__main__":
    unittest.main()
