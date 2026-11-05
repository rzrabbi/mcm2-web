# Online multiplayer

This document explains online network play for the browser build. The static web build
contains only the client-side game engine—it does not include or connect to a
central multiplayer server by default. To race with friends online, you or someone in your group
runs a lightweight **relay server** (`tools/relay`, binary `mm2-relay`), because web browsers cannot
listen for raw TCP/UDP connections directly.

> **Important:** Every player must load their own copy of Motocross Madness 2 into their browser.
> The host does **not** share, stream, or distribute its game files to guests. The relay server
> acts solely as an encrypted packet forwarder for player position and race events; no game assets,
> textures, or tracks ever touch the relay.

```
 your browser ──WebSocket──┐
                           ├── relay (tools/relay, mm2-relay) ──┐
 a friend's browser ───────┘                                  └─ passes packets, never game files
```

## How it plays

The game's network mode is a star: one player is the host, up to seven others connect to the
host, and the host passes on what each sends. The relay stands in for the local network.

- **Host a room:** open the **Multiplayer** tab on the start page, give the room a name and
  press **Create room**. It shows a five-character code to share. Tick *Show in the server
  browser* to make it public. The room has 0 players and nobody can join it until you press
  **Enter lobby**, which starts the game and opens its lobby. A room nobody enters is deleted
  after 5 minutes. If you leave the lobby or reload the page the room is kept for you, with
  its code and settings, for the same time.
- **Join a room:** pick one in the server browser and press **Join**, or type its code. The
  game opens straight into the lobby and joins from there, so you are counted only once you
  are in it.
- **In the game:** Multiplayer, then TCP/IP. **Internet** is the server browser: it lists the
  public rooms, and Host makes a public room. **LAN** asks for a room code first and joins
  that private room; Host there makes a private room that is never listed.
- **Starting a race:** the host presses the flag once everyone is ready. Every player's game
  checks that it has the track the host picked (each player has their own copy of the game, so a
  different version may lack it) and waits on a loading screen. The race then starts with the
  players who have it. A player who lacks the track, has left or does not answer within 90
  seconds is left out, and the chat says so.
- Everyone needs the game folder loaded (press **Start game**, or let **Join** / **Enter
  lobby** do it) and, to race on the same tracks, the same version of the game.

## Setting it up

The relay is a small Rust server (`mm2-relay` in `tools/relay`) built with `tokio` and `tokio-tungstenite`. It
keeps rooms in memory, stores nothing and has no accounts. You need to run one for the people
you want to play with.

### Try it on one machine

```sh
cargo run --release --bin mm2-relay    # listens on 127.0.0.1:2108

cd crates/mm2-viewer
trunk serve --port 8080                # the page, at http://127.0.0.1:8080
```

A page on `localhost` finds a relay on port 2108 by itself. Open the page in two browser
windows (a hidden tab pauses the game) and host in one and join in the other.

### Play with friends over the internet

Put the relay where everyone can reach it and give it HTTPS, because a page served over HTTPS
(Cloudflare Pages, GitHub Pages, or any secure host) can only open a secure `wss://` socket.

1. **Run the relay** on a server or VPS, with Docker:

   ```sh
   cd tools/relay
   docker compose up -d --build
   ```

   or without it: `cargo run --release --bin mm2-relay -- --host 127.0.0.1 --port 2108`.

2. **Add HTTPS with a reverse proxy** that forwards WebSocket traffic to port 2108.
   Any reverse proxy or proxy manager (Caddy, Nginx, Apache, Traefik, HAProxy, NPM, etc.)
   works.

   - **WebSocket upgrades:** Ensure HTTP/1.1 and WebSocket upgrade headers are passed to the relay.
     If configuring upgrade headers manually (e.g. Nginx or proxy managers without a one-click
     WebSocket toggle), use a dynamic connection upgrade header rather than a static `"upgrade"`
     string, so regular HTTP requests like `/mp/stats` and `/mp/rooms` keep standard keep-alive
     connections:
     ```nginx
     proxy_http_version 1.1;
     proxy_set_header Upgrade $http_upgrade;
     proxy_set_header Connection $connection_upgrade;
     proxy_set_header Host $host;
     ```
   - **Hosting the relay under a subpath (e.g. `/mp/`):**
     If mounting the relay under a subpath on the same domain as the game, always specify the route
     with a trailing slash (`/mp/`, never `/mp`). In many proxies, a prefix rule for `/mp` will
     also match the start page's script `/mp.js`, inadvertently routing that script to the relay
     and breaking page startup (`mm2NetWantLobby is not defined`).
   - **Caddy example:**
     ```
     relay.example.com {
         reverse_proxy 127.0.0.1:2108
     }
     ```
   - **CDNs and edge proxies (e.g. Cloudflare):**
     If proxying through an edge CDN, verify that WebSocket connections and long session timeouts
     are supported. If connection limits or timeouts cause disconnects during races, bypass edge
     proxying (DNS-only) for the relay hostname.
   - **Client IP forwarding:**
     Set `TRUST_PROXY=1` for the relay (the compose file does) so it takes each player's address
     from the proxy's `X-Forwarded-For` header.

3. **Tell the relay which page may use it.** A page on another host than the relay is refused
   unless its origin is listed. Set `ALLOWED_ORIGINS` to the page's origin, for example
   `https://you.github.io`, in `tools/relay/compose.yaml` (comma separate several).

4. **Point the page at the relay.** Any one of these works, in this order of priority:
   - add `?relay=wss://relay.example.com` to the page's address (share that link);
   - open **Multiplayer**, then *Relay server*, enter `wss://relay.example.com` and press
     **Save** (it is remembered in that browser);
   - for your own deployment, set `<meta name="mm2-relay" content="wss://relay.example.com" />`
     in `crates/mm2-viewer/index.html`, so everyone who opens it has the relay already.
     `content="/mp"` means a relay behind the page's own server at `/mp`.

   Everyone must use the same relay.

### Settings

| What | Where | Default |
| --- | --- | --- |
| `--host`, `--port` | relay command line (or `HOST` in the environment) | `127.0.0.1`, `2108` |
| `TRUST_PROXY=1` | relay environment | off |
| `ALLOWED_ORIGINS` | relay environment, comma separated | this host and localhost only |
| `PARK_SECONDS` | relay environment | `300` (how long a room nobody entered is kept) |

The relay also limits rooms and connections per address and the rate of packets, so an open
relay cannot be used up easily; the numbers are in `Limits` in `tools/relay/src/lib.rs`.

## What the relay sees

Room codes and names, the players' chosen names and bikes, and the game's network packets
(bike positions, chat). It does not see game files, saves or anything else from your browser.
Anyone who knows a room's code can join it, so a private room is as private as its code.

## Troubleshooting

- **"The relay is not reachable":** check the address, that the relay is running, and for a
  page on HTTPS that the address starts with `wss://` and its certificate is valid.
- **"mm2NetWantLobby is not defined" or 502 on `mp.js`:** The reverse proxy route was configured as
  `/mp` instead of `/mp/`. The proxy intercepted `/mp.js` and routed the client script to the relay
  instead of the web server. Change the reverse proxy route to `/mp/`.
- **The relay refuses the connection ("origin not allowed"):** add the page's origin to
  `ALLOWED_ORIGINS`.
- **Relay crashes with `MultipleValuesError` on `X-Forwarded-For`:** Occurs if an upstream proxy
  passes multiple `X-Forwarded-For` header lines. Handled in `mm2-relay` by aggregating all forwarded
  header lines.
- **502 Bad Gateway on `/mp/stats`:** Check that the proxy's `Connection` header is dynamic
  (`$connection_upgrade`) rather than statically set to `"upgrade"` on non-WebSocket HTTP requests.
- **"No room has that code" / "The host has not opened the lobby yet":** the host has not pressed
  **Enter lobby** yet, or the room was deleted after 5 minutes.
- **A player is left out of the race:** their game does not have the host's track. Check that
  everyone has the same version of the game.
- **The lobby stops updating:** browsers pause a hidden tab. Keep the game's tab in view.

## How it is built

| Part | Where |
| --- | --- |
| Relay: rooms, codes, listing, limits | `tools/relay` (`mm2-relay`, tests in `tools/relay/src/lib.rs`) |
| Page: the Multiplayer tab and the relay connection | `crates/mm2-viewer/mp.js`, `index.html` |
| Engine: network session over the relay, lobby and race start | `crates/mm2-viewer/src/net.rs` (`mod web`), `screens/multi.rs`, `frontend.rs` |

The session logic (host, guests, lobby, whole-bike packets) is the engine's own network code.
In the browser it runs over the relay instead of TCP and UDP: `net.rs` has a `wasm32` transport
that calls the `mm2Net*` functions `mp.js` defines. `tools/relay/src/lib.rs` documents the relay's
messages at the top of the file.
