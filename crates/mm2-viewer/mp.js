// Online multiplayer for the start page: the room (through a relay server) and the Multiplayer tab.
//
// A browser cannot listen for connections, so network play goes through a small WebSocket relay
// (`tools/relay/relay.py`): one player's browser hosts a room, the others join it by its code, and
// the relay passes the game's packets between them. The relay is yours to run; see
// docs/multiplayer.md. The page learns its address from, in order: `?relay=` in the page's
// address, the address saved in this browser (Multiplayer tab), `<meta name="mm2-relay">` in
// index.html, and on localhost `localhost:2108`.
//
// `window.mm2net` is the one place that talks to the relay. The tab uses it, and so does the
// engine (`mod web` in crates/mm2-viewer/src/net.rs, through the `mm2Net*` functions below), so a
// room made here is still open when the game starts:
//
//   mm2net.host({name, public, info, parked}) -> Promise<{code}>   open a room, as its host
//   mm2net.join(code)                 -> Promise<{code, id, name}>
//   mm2net.send(id, bytes)            host: to guest `id` (255 = all guests); guest: to the host
//   mm2net.recv()                     -> next event or null:
//                                        {peer, up}      host: a guest came or went
//                                        {from, data}    a frame (from: guest id, or 0 = host)
//                                        {lost}          the room is gone
//   mm2net.setInfo({info, public, name})  host: what the server browser shows
//   mm2net.leave()
//   mm2net.state  "idle" | "hosting" | "joined",  mm2net.room  {code, name, id}
//   mm2net.on(fn)  fn() runs when state or room changes
//
// A room made on the start page is "parked" until its host is in the game's lobby: it has no
// players, nobody can join it and it is not listed. The page keeps its code and token in
// localStorage, so a reload takes the room back (resume); a room nobody enters is deleted by the
// relay.
//   mm2net.parked                    hosting, but not in the game's lobby
//   mm2net.enter() / mm2net.park()   the host is in / has left the game's lobby
//   mm2net.resume()                  take back the room kept in this browser
//   mm2net.close()                   delete the room (the "Leave room" button)
//   mm2net.expiresAt                 when a parked room is deleted (ms), or 0
(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const LOCAL = /^(localhost|127\.0\.0\.1|\[::1\])(:|\/|$)/;

  // ---- which relay -----------------------------------------------------------------------

  const RELAY_KEY = "mm2-relay";
  const HOST_KEY = "mm2-host";
  const store = {
    get(key) { try { return localStorage.getItem(key); } catch { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch {} },
    remove(key) { try { localStorage.removeItem(key); } catch {} },
  };

  // An address as typed ("host:2108", "wss://example.com/mp", "/mp") into the parts the page
  // needs, or null if it is not one.
  const parseRelay = (raw) => {
    let s = String(raw || "").trim().replace(/\/+$/, "");
    if (!s) return null;
    let secure;
    const given = s.match(/^(wss?|https?):\/\/(.*)$/i);
    if (given) {
      secure = /^(wss|https)$/i.test(given[1]);
      s = given[2];
    } else if (s.startsWith("/")) {
      s = location.host + s;                       // the relay behind this page's own server
      secure = location.protocol === "https:";
    } else {
      // A secure page can only use a secure socket, except to this machine.
      secure = location.protocol === "https:" && !LOCAL.test(s);
    }
    if (s.endsWith("/ws")) s = s.slice(0, -3);     // a pasted socket address works too
    if (!/^[\w.\-\[\]:]+(\/[\w.\-\/]*)?$/.test(s)) return null;
    const ws = `${secure ? "wss" : "ws"}://${s}`;
    const http = `${secure ? "https" : "http"}://${s}`;
    // https page + ws:// to another machine: the browser would block it.
    const blocked = location.protocol === "https:" && ws.startsWith("ws://") && !LOCAL.test(s);
    return { text: String(raw).trim(), wsUrl: ws + "/ws", http, blocked };
  };

  const chosenRelay = () => {
    const param = new URLSearchParams(location.search).get("relay");
    const meta = document.querySelector('meta[name="mm2-relay"]');
    const local = LOCAL.test(location.host) ? `${location.hostname}:2108` : "";
    return param || store.get(RELAY_KEY) || (meta && meta.content) || local;
  };
  let relay = parseRelay(chosenRelay());

  const NO_RELAY = "No relay server is set. Enter its address under Multiplayer.";
  const httpUrl = (path) => {
    if (!relay) throw new Error(NO_RELAY);
    return relay.http + path;
  };

  // ---- the room kept for this browser's host ---------------------------------------------

  // What is kept is tied to the relay it was made on: another relay does not know the room.
  const stored = {
    get() {
      try {
        const rec = JSON.parse(store.get(HOST_KEY) || "null");
        return rec && relay && rec.relay === relay.text ? rec : null;
      } catch { return null; }
    },
    set(v) { store.set(HOST_KEY, JSON.stringify({ ...v, relay: relay ? relay.text : "" })); },
    clear() { store.remove(HOST_KEY); },
  };
  // The tab that hosts holds this lock, so a second tab of the same browser does not take the
  // room (its token is shared through localStorage) from under it when it loads.
  const LOCK = "mm2-host-room";
  let releaseLock = null;
  const holdRoom = () => {
    if (!navigator.locks || releaseLock) return;
    navigator.locks.request(LOCK, () => new Promise((done) => { releaseLock = done; })).catch(() => {});
  };
  const dropRoom = () => { if (releaseLock) { releaseLock(); releaseLock = null; } };

  // ---- the connection --------------------------------------------------------------------

  const listeners = new Set();
  const queue = [];
  let ws = null;
  const net = {
    state: "idle",
    room: null,
    lost: "",           // why the last room ended: "host_left", "expired" or "closed"
    busy: false,        // a host or join is waiting for the relay
    parked: false,      // hosting, but not in the game's lobby: nobody can join yet
    expiresAt: 0,       // when a parked room is deleted (ms since 1970)
    wantLobby: false,   // the start page asked the game to open the lobby straight away
    pendingJoin: "",    // a room code the game joins once it is in its lobby (nobody is counted before)
    error: "",          // why the last host or join failed
    on: (fn) => listeners.add(fn),
    changed() {
      if (net.state === "hosting") holdRoom(); else dropRoom();
      for (const fn of listeners) { try { fn(); } catch (e) { console.error(e); } }
    },
    recv: () => queue.shift() || null,
    stats: () => fetch(httpUrl("/stats"), { cache: "no-store" }).then(ok),
    rooms: () => fetch(httpUrl("/rooms"), { cache: "no-store" }).then(ok).then((j) => j.rooms),
    get relay() { return relay; },
  };
  const ok = (r) => { if (!r.ok) throw new Error("relay " + r.status); return r.json(); };

  const closeSocket = () => {
    const old = ws;
    ws = null;
    if (old) { old.onclose = old.onmessage = old.onerror = null; try { old.close(); } catch {} }
  };

  // Opens the socket, sends `first`, and resolves with the relay's reply to it.
  const connect = (first) => new Promise((resolve, reject) => {
    if (net.state !== "idle") return reject(new Error("Already in a room."));
    if (!relay) return reject(new Error(NO_RELAY));
    if (relay.blocked) return reject(new Error("This page is secure, so the relay address must start with wss://."));
    let sock;
    try { sock = new WebSocket(relay.wsUrl); } catch (e) { return reject(new Error("The relay is not reachable.")); }
    sock.binaryType = "arraybuffer";
    ws = sock;
    queue.length = 0;
    net.lost = "";
    let settled = false;
    const fail = (message) => { if (!settled) { settled = true; clearTimeout(timer); closeSocket(); reject(new Error(message)); } };
    const timer = setTimeout(() => fail("The relay did not answer."), 8000);
    sock.onopen = () => sock.send(JSON.stringify(first));
    sock.onerror = () => fail("The relay is not reachable.");
    sock.onmessage = (e) => {
      if (typeof e.data !== "string") {
        if (settled && net.state !== "idle") {
          const bytes = new Uint8Array(e.data);
          // The host sees which guest sent it; a guest only ever hears the host.
          if (net.state === "hosting") queue.push({ from: bytes[0], data: bytes.subarray(1) });
          else queue.push({ from: 0, data: bytes });
        }
        return;
      }
      let m;
      try { m = JSON.parse(e.data); } catch { return; }
      if (!settled) {
        if (m.t === "error") return fail(REASONS[m.reason] || "The relay refused.");
        if (m.t !== "hosted" && m.t !== "joined") return;
        settled = true;
        clearTimeout(timer);
        net.state = m.t === "hosted" ? "hosting" : "joined";
        net.room = { code: m.code, name: m.name || first.name || "", id: m.id ?? 0 };
        if (m.t === "hosted") {
          net.parked = !m.entered;
          net.expiresAt = net.parked ? Date.now() + (m.ttl || 300) * 1000 : 0;
          net.ttl = m.ttl || 300;
          if (m.token) stored.set({ code: m.code, token: m.token, name: net.room.name, public: !!m.public });
        }
        net.changed();
        resolve(net.room);
      } else if (m.t === "peer") {
        queue.push({ peer: m.id, up: m.up });
      }
    };
    sock.onclose = (e) => {
      if (!settled) return fail(e.code === 1013 ? REASONS.busy : "The relay is not reachable.");
      ws = null;
      const wasIn = net.state !== "idle";
      net.state = "idle";
      net.parked = false;
      net.expiresAt = 0;
      // The relay deleted a room nobody entered in time.
      if (e.code === 4003) stored.clear();
      if (wasIn) {
        net.lost = e.code === 4000 ? "host_left" : e.code === 4003 ? "expired" : "closed";
        queue.push({ lost: net.lost });
        net.changed();
      }
    };
  });

  const REASONS = {
    no_room: "No room has that code.",
    not_open: "The host has not opened the lobby yet. Try again in a moment.",
    full: "That room is full.",
    busy: "The relay is busy. Try again in a moment.",
    rate: "Too many wrong codes. Wait a minute.",
    bad_request: "The relay did not understand that.",
  };

  // `busy` is on from the call until the relay answers, so the game can tell "waiting" from "idle".
  const start = (first) => {
    net.busy = true;
    net.error = "";
    return connect(first).catch((e) => { net.error = e.message; throw e; }).finally(() => { net.busy = false; });
  };
  // `parked`: made on the start page, opened when its host is in the game's lobby.
  net.host = ({ name = "", public: open = false, info = "", parked = false } = {}) => start({ t: "host", name, public: open, info, parked });
  // Takes back the room this browser made (after a reload, or the connection went). One at a
  // time: the page's own on load and the game's (Enter lobby) share it, or the second would take
  // the room from the first.
  let resuming = null;
  net.resume = () => {
    if (resuming) return resuming;
    const mine = stored.get();
    if (!mine) return Promise.reject(new Error("No room to resume."));
    resuming = start({ t: "resume", code: mine.code, token: mine.token }).catch((e) => {
      if (net.error === REASONS.no_room) stored.clear();     // it expired, or the relay restarted
      throw e;
    }).finally(() => { resuming = null; });
    return resuming;
  };
  net.mine = () => stored.get();
  // The host is in the game's lobby: the room opens (and is taken back first if need be).
  net.enter = async () => {
    net.wantLobby = false;
    if (resuming) await resuming.catch(() => {});
    if (net.state === "idle" && stored.get()) await net.resume();
    if (net.state !== "hosting" || !ws) throw new Error("No room to enter.");
    ws.send(JSON.stringify({ t: "enter" }));
    net.parked = false;
    net.expiresAt = 0;
    net.changed();
  };
  // The host left the game's lobby: others are sent out; the room waits for its host.
  net.park = () => {
    if (net.state !== "hosting" || !ws) return;
    ws.send(JSON.stringify({ t: "park" }));
    net.parked = true;
    net.expiresAt = Date.now() + (net.ttl || 300) * 1000;
    net.changed();
  };
  // Lets go of a room nobody has entered, to join another: the relay deletes it when its time is up.
  net.detach = () => {
    if (net.state !== "hosting" || !net.parked) return;
    closeSocket();
    queue.length = 0;
    net.state = "idle";
    net.room = null;
    net.parked = false;
    net.expiresAt = 0;
    net.changed();
  };
  // Deletes the room for good.
  net.close = () => {
    if (net.state === "hosting" && ws) { try { ws.send(JSON.stringify({ t: "close" })); } catch {} }
    stored.clear();
    net.leave();
  };
  net.join = (code) => start({ t: "join", code: String(code).trim() });
  net.setInfo = (fields) => { if (net.state === "hosting" && ws) ws.send(JSON.stringify({ t: "info", ...fields })); };
  net.send = (id, data) => {
    if (!ws || ws.readyState !== 1) return false;
    if (net.state === "hosting") {
      const frame = new Uint8Array(data.length + 1);
      frame[0] = id;
      frame.set(data, 1);
      ws.send(frame);
    } else {
      ws.send(data);
    }
    return true;
  };
  net.leave = () => {
    const was = net.state !== "idle";
    closeSocket();
    queue.length = 0;
    net.state = "idle";
    net.room = null;
    net.parked = false;
    net.expiresAt = 0;
    if (was) net.changed();
  };
  // Another relay: any room here is left (a parked one stays on the old relay until it lapses).
  net.setRelay = (raw) => {
    const next = parseRelay(raw);
    if (String(raw || "").trim() && !next) throw new Error("That is not a relay address.");
    if (net.state !== "idle") net.leave();
    relay = next;
    if (next) store.set(RELAY_KEY, next.text); else store.remove(RELAY_KEY);
    net.changed();
  };
  window.mm2net = net;

  // What the engine calls (net.rs, `mod web`): plain functions with simple arguments.
  let roomLines = "", roomsAt = 0;
  Object.assign(window, {
    mm2NetState: () => (net.busy ? "connecting" : net.state),
    mm2NetCode: () => (net.room ? net.room.code : ""),
    mm2NetError: () => net.error,
    mm2NetHost: (name, open) => { net.host({ name, public: open }).catch(() => {}); },
    mm2NetJoin: (code) => { net.join(code).catch(() => {}); },
    mm2NetLeave: () => net.leave(),
    // A host in the game's lobby: the room opens. Leaving the lobby parks it again.
    mm2NetEnter: () => { net.enter().catch(() => {}); },
    mm2NetPark: () => net.park(),
    mm2NetDetach: () => net.detach(),
    mm2NetParked: () => net.state === "hosting" && net.parked,
    // The room this browser keeps for its host (whether or not it is connected).
    mm2NetOwns: (code) => { const m = stored.get(); return !!m && m.code === String(code).trim().toUpperCase(); },
    // True once if the start page asked for the lobby straight away (to host or to join).
    mm2NetWantLobby: () => { const w = net.wantLobby || !!net.pendingJoin; net.wantLobby = false; return w; },
    // The room code Join was pressed on, once: the game joins it from its lobby.
    mm2NetPendingJoin: () => { const c = net.pendingJoin; net.pendingJoin = ""; return c; },
    mm2NetSend: (id, data) => net.send(id, data),
    // kind (1 a player came, 2 went, 3 a frame, 4 the room is gone), the player's number, the frame.
    mm2NetNext: () => {
      const e = net.recv();
      if (!e) return new Uint8Array(0);
      if (e.data) {
        const out = new Uint8Array(e.data.length + 2);
        out[0] = 3;
        out[1] = e.from;
        out.set(e.data, 2);
        return out;
      }
      if (e.peer !== undefined) return Uint8Array.of(e.up ? 1 : 2, e.peer);
      return Uint8Array.of(4, 0);
    },
    // The public rooms as lines of code, name, notes and players; refreshed at most every 4 s.
    mm2NetRooms: () => {
      if (Date.now() - roomsAt > 4000) {
        roomsAt = Date.now();
        net.rooms().then((rooms) => {
          const line = (r) => [r.code, r.name, r.info, `${r.players}/${r.max}`].join(String.fromCharCode(9));
          roomLines = rooms.map(line).join(String.fromCharCode(10));
        }, () => { roomLines = ""; });
      }
      // The host's own room, not open to others yet, is listed for the host alone.
      const mine = stored.get();
      if (mine && !(net.state === "hosting" && !net.parked)) {
        const own = [mine.code, "* " + mine.name, "your room", "0/8"].join(String.fromCharCode(9));
        return own + (roomLines ? String.fromCharCode(10) + roomLines : "");
      }
      return roomLines;
    },
  });

  // ---- the Multiplayer tab ---------------------------------------------------------------

  if (!$("mp")) return;
  const el = {
    stats: $("mp-stats"), room: $("mp-room"), code: $("mp-code"), roomName: $("mp-room-name"), copy: $("mp-copy"),
    leave: $("mp-leave"), enter: $("mp-enter"), forms: $("mp-forms"), name: $("mp-name"), open: $("mp-public"), create: $("mp-create"),
    popup: $("mp-popup"), popupText: $("mp-popup-text"), popupCancel: $("mp-popup-cancel"),
    joinbox: $("mp-joinbox"), joinCode: $("mp-join-code"), join: $("mp-join"), list: $("mp-list"), refresh: $("mp-refresh"),
    msg: $("mp-msg"), relayBox: $("mp-relay-box"), relayName: $("mp-relay-name"), relayInput: $("mp-relay"),
    relaySave: $("mp-relay-save"),
  };
  const say = (text, bad) => { el.msg.textContent = text || ""; el.msg.classList.toggle("err", !!bad); };
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  const showStats = (s, why) => {
    el.stats.textContent = s
      ? `${plural(s.public_rooms, "public room", "public rooms")} · ${plural(s.players, "player", "players")} online`
      : why || "Server offline";
    el.stats.classList.toggle("off", !s);
  };

  // A player may join another room while hosting one nobody has entered (it is deleted in time).
  const canJoin = () => net.state === "idle" || (net.state === "hosting" && net.parked);
  const row = (r) => {
    const li = document.createElement("li");
    const who = document.createElement("div");
    who.className = "who";
    const b = document.createElement("b");
    b.textContent = r.name;
    const small = document.createElement("small");
    small.textContent = `${r.players}/${r.max} players${r.info ? " · " + r.info : ""} · ${r.code}`;
    who.append(b, small);
    const go = document.createElement("button");
    go.type = "button";
    go.className = "link";
    go.textContent = r.players >= r.max ? "Full" : "Join";
    go.disabled = r.players >= r.max || !canJoin();
    go.addEventListener("click", () => joinLobby(r.code));
    li.append(who, go);
    return li;
  };

  const showRooms = (rooms) => {
    el.list.replaceChildren();
    if (!rooms || !rooms.length) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = rooms ? "No public rooms right now. Make one, or join with a code."
        : relay ? "The room list is not available." : "Set a relay server to see rooms.";
      el.list.append(li);
      return;
    }
    el.list.append(...rooms.map(row));
  };

  let polling = 0;
  const refresh = async () => {
    if (!relay) { showStats(null, "No relay server set"); showRooms(null); return; }
    const [s, r] = await Promise.allSettled([net.stats(), net.rooms()]);
    showStats(s.status === "fulfilled" ? s.value : null);
    showRooms(r.status === "fulfilled" ? r.value : null);
  };
  const visible = () => $("mp").classList.contains("on") && !document.hidden;
  const poll = () => { if (visible()) refresh(); polling = setTimeout(poll, 3000); };

  const minutes = (ms) => Math.max(1, Math.ceil(ms / 60000));
  const paint = () => {
    const inRoom = net.state !== "idle";
    // A room kept for this host, with no connection yet (just after a reload).
    const waiting = !inRoom && net.busy && !!net.mine();
    el.room.hidden = !inRoom && !waiting;
    el.forms.hidden = inRoom || waiting;
    el.joinbox.hidden = !canJoin() || waiting;
    el.enter.hidden = !(inRoom && net.state === "hosting");
    el.relayName.textContent = relay ? relay.text : "not set";
    if (!relay) el.relayBox.open = true;
    if (waiting) {
      el.code.textContent = net.mine().code;
      el.roomName.textContent = "Taking your room back…";
    } else if (inRoom) {
      el.code.textContent = net.room.code;
      if (net.state === "joined") {
        el.roomName.textContent = `You joined “${net.room.name || "Room"}”. Keep this page open and press Start game.`;
      } else if (net.parked) {
        el.roomName.textContent = `“${net.room.name || "Room"}” has 0 players. Nobody can join, and it is not listed, until you enter the lobby. `
          + `It is deleted in ${minutes(net.expiresAt - Date.now())} min if you do not.`;
      } else {
        el.roomName.textContent = `You are hosting “${net.room.name || "Room"}”. Keep this page open.`;
      }
    }
    for (const b of el.list.querySelectorAll("button")) b.disabled = !canJoin() || b.textContent === "Full";
  };
  setInterval(() => { if (net.parked) paint(); }, 15000);
  net.on(() => {
    paint();
    if (net.state === "idle") {
      if (net.lost) {
        say(net.lost === "host_left" ? "The host left the lobby."
          : net.lost === "expired" ? "Your room was deleted: you did not enter the lobby in time."
          : "Lost the connection to the relay.", true);
      }
      refresh();
    }
  });

  // "Joining the lobby": the game starts and goes straight to its lobby, and Join happens there,
  // so a player is counted only once they are in it. The game must be loaded first: the page can
  // press Start game, but only the player can choose the game folder.
  let popupTimer = 0, popupGuard = 0;
  const startButton = () => $("start-game");
  const showPopup = (on) => {
    el.popup.hidden = !on;
    clearTimeout(popupTimer);
    clearTimeout(popupGuard);
  };
  const cancelPopup = () => { showPopup(false); net.pendingJoin = ""; net.wantLobby = false; };
  // The popup stays up at least this long, so it is seen even when the game starts at once.
  const POPUP_MIN_MS = 900;
  const showJoining = () => {
    showPopup(true);
    el.popupText.textContent = "Joining the lobby…";
    return Date.now();
  };
  const pressStart = (shownAt) => {
    const press = () => {
      if (el.popup.hidden) return;                       // cancelled
      const wait = POPUP_MIN_MS - (Date.now() - shownAt);
      const go = startButton();
      if (wait > 0) popupTimer = setTimeout(press, wait);
      else if (go) {
        go.click();                                      // the game takes over from here
        // It did not (the saves could not be read, or the game failed to start): stop waiting.
        popupGuard = setTimeout(() => { if (!el.popup.hidden) cancelPopup(); }, 30000);
      } else popupTimer = setTimeout(press, 300);
    };
    press();
  };
  // Whether the game is loaded; if not, says what to do.
  const gameReady = () => {
    if (startButton()) return true;
    say("Choose your game folder first (above), then press this again.", true);
    return false;
  };
  const goLobby = () => { if (gameReady()) pressStart(showJoining()); };
  const joinLobby = async (code) => {
    say("");
    const parked = net.state === "hosting" && net.parked;
    if (net.state !== "idle" && !parked) return say("Leave your room first.", true);
    if (!gameReady()) return;
    const shownAt = showJoining();
    // A room can fill up or close while the list is on screen: look again before leaving the page.
    try {
      const rooms = await net.rooms();
      const room = rooms.find((r) => r.code === code);
      if (!room) { cancelPopup(); refresh(); return say("That room is no longer open.", true); }
      if (room.players >= room.max) { cancelPopup(); refresh(); return say("That room is full.", true); }
    } catch { /* the relay will say it, from the game */ }
    if (el.popup.hidden) return;                         // cancelled meanwhile
    if (parked) net.detach();                            // the unentered room of this player lapses
    net.pendingJoin = code;
    pressStart(shownAt);
  };
  el.popupCancel.addEventListener("click", cancelPopup);

  const enter = async (start) => {
    say("");
    for (const b of [el.create, el.join]) b.disabled = true;
    try { await start(); } catch (e) { say(e.message, true); }
    for (const b of [el.create, el.join]) b.disabled = false;
    refresh();
  };

  // A room made here waits (parked) until its host is in the game's lobby.
  el.create.addEventListener("click", () => enter(() => net.host({ name: el.name.value.trim() || "Moto Room", public: el.open.checked, parked: true })));
  el.join.addEventListener("click", () => {
    const code = el.joinCode.value.trim();
    if (!code) return say("Type a room code first.", true);
    const parked = net.state === "hosting" && net.parked;
    if (net.state !== "idle" && !parked) return say("Leave your room first.", true);
    if (!gameReady()) return;
    // The room is not joined here: the game does it from its lobby.
    const shownAt = showJoining();
    if (parked) net.detach();
    net.pendingJoin = code.toUpperCase();
    pressStart(shownAt);
  });
  el.joinCode.addEventListener("keydown", (e) => { if (e.key === "Enter") el.join.click(); });
  el.joinCode.addEventListener("input", () => { el.joinCode.value = el.joinCode.value.toUpperCase().replace(/[^A-Z0-9]/g, ""); });
  el.leave.addEventListener("click", () => { if (net.state === "hosting") net.close(); else net.leave(); say(""); });
  // Press Start game for the host: the game opens its lobby straight away.
  el.enter.addEventListener("click", () => {
    say("");
    net.wantLobby = true;
    goLobby();
    if (el.popup.hidden) net.wantLobby = false;          // the game was not ready
  });
  el.refresh.addEventListener("click", refresh);
  el.copy.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(net.room.code);
      say("Code copied.");
    } catch {
      say("Select the code and copy it.", true);
    }
  });

  // The relay's address.
  el.relayInput.value = relay ? relay.text : "";
  el.relayBox.open = !relay;
  el.relaySave.addEventListener("click", () => {
    say("");
    try {
      net.setRelay(el.relayInput.value);
    } catch (e) {
      return say(e.message, true);
    }
    el.relayInput.value = relay ? relay.text : "";
    paint();
    refresh();
    // A room kept for this relay, if it had one.
    if (relay && stored.get()) net.resume().catch(() => {});
  });
  el.relayInput.addEventListener("keydown", (e) => { if (e.key === "Enter") el.relaySave.click(); });

  document.addEventListener("visibilitychange", () => { if (visible()) refresh(); });
  $("tab-mp").addEventListener("click", refresh);
  // Closing or reloading the page leaves a guest's room, but a host's room is kept for it
  // (the relay holds it for a while): the page takes it back when it loads again.
  addEventListener("pagehide", () => { if (net.state === "joined") net.leave(); else closeSocket(); });
  // A room this browser made is taken back on load, unless another tab of it is hosting now.
  (async () => {
    if (!stored.get()) return;
    try {
      if (navigator.locks && (await navigator.locks.query()).held.some((l) => l.name === LOCK)) return;
    } catch { /* no lock list: go ahead */ }
    net.resume().catch(() => {});
  })();
  paint();
  poll();
})();
