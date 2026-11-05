# CLAUDE.md

Browser port of Motocross Madness 2: a Rust engine (from
[openmcm2](https://github.com/crspytopgn/openmcm2)) compiled to WebAssembly and drawn
with WebGPU. Players choose their own installed copy of the game in the page. Nothing is
uploaded. The same crate also builds as a desktop viewer.

Engine baseline: openmcm2 v0.2.1 (commit `046c300`). The crate version `0.2.1` is
upstream's, not this port's.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for the full picture: intent, how it works, what
was done, what works and what does not, known issues, and licences. Read it before large
changes.

## Rules

- **Never commit game files**, anything extracted from them, or texture packs made from
  them. `.gitignore` blocks the game's formats. Never ship game art in the page; build
  anything drawn from the game (such as the pointer) at run time from the player's files.
- **Never paste decompiled code** or extracted data. Describe findings in your own words
  and cite addresses.
- **Keep the LGPL boundary:** changes in `crates/mm2-indeo` stay under the LGPL 2.1 or
  later. Everything else is MIT OR Apache-2.0.
- **Keep the page self-contained:** no external scripts or services. Online multiplayer talks
  only to an optional relay server the player runs or connects to (`mp.js` is the only code that does);
  nothing of the game goes to it.
- Differences between desktop and browser go through `crates/mm2-viewer/src/platform.rs`
  or a `cfg!(target_arch = "wasm32")` constant, so desktop behaviour stays unchanged.
- Match the surrounding code's style, including its comments (plain sentences saying what
  the original game does and at which address).
- Do not push or commit unless asked.

## Commands

```sh
cargo test --release --workspace                      # 164 tests; game-data tests skip without MM2_DATA
cargo check --release --target wasm32-unknown-unknown -p mm2-viewer \
  --no-default-features --features audio,gamepad      # browser build check

cd crates/mm2-viewer
trunk serve --port 8080                               # dev server (unoptimised wasm)
trunk build --release                                 # optimised build in dist/ (runs wasm-opt)

cargo run --release --bin serve-game -- "<game folder>" --port 8081  # then open :8080/?game=http://127.0.0.1:8081
cargo run --release --bin mm2-relay                   # the multiplayer relay (a page on localhost finds :2108)
cargo test --release -p mm2-relay                     # the relay's tests
cargo run --release -p mm2-viewer -- "<game folder>"    # desktop viewer
```

## Where things are

- `crates/mm2-viewer/index.html`: the web page (folder picker, the Multiplayer, Graphics and Profiles tabs, sound, pointer).
- `crates/mm2-viewer/mp.js`: the Multiplayer tab and the relay connection (`window.mm2net`, the `mm2Net*` functions the engine calls).
- `crates/mm2-viewer/src/net.rs`, `screens/multi.rs`: network session (a WebSocket transport in `mod web`) and the lobby.
- `tools/relay/`: the relay server and its tests. `docs/multiplayer.md`: setup and how it works.
- `crates/mm2-viewer/src/platform.rs`: in-memory file system and browser shims.
- `crates/mm2-viewer/src/main.rs`: entry points, including the `web` module (wasm API).
- `crates/mm2-viewer/src/frontend.rs`, `ui.rs`, `sprites.rs`: menus and layout.
- `crates/mm2-physics`, `mm2-formats`, `mm2-indeo` (video), `mm2-tools`: libraries and tools.

## Rust Tooling & Server Architecture (Switched from Python to Rust)

All companion tooling and servers (`serve-game` and `mm2-relay`) are written in Rust rather than Python. Do not introduce Python scripts or dependencies back into the repository.
- **`serve-game` (`crates/mm2-tools/src/serve.rs`):** Replaced legacy `tools/serve_game.py`. Pre-caches the file manifest and remote index at startup and serves HTTP/1.1 with keep-alive and `TCP_NODELAY`, eliminating slow file-tree walks on each request and preventing TCP connection handshakes when the browser loads assets.
- **`mm2-relay` (`tools/relay/src/`):** Replaced legacy `tools/relay/relay.py`. Uses `tokio` and `tokio-tungstenite`. Idle RAM dropped from ~30 MB to ~5 MB, CPU overhead and event loop latency under multi-room load are significantly reduced, and per-client frame queues are bounded to prevent memory bloat on low-resource VPS hosts. Also produces a tiny scratch/distroless multi-stage Docker container.
- **Broadcast optimization:** `Session::broadcast` in `crates/mm2-viewer/src/net.rs` uses relay-level broadcast targets (`255` for all guests, `0x80 | id` for all guests except sender). The host sends only one WebSocket packet per broadcast rather than N individual unicast frames.

## Known open issues

See ARCHITECTURE.md §8. In short: a failed track fetch leaves that event stuck until
reload; two quick event opens overlap their fetches; the loader path for the folder picker
(`TERAFORM` fix) still needs a browser test. Online multiplayer needs a relay you run (docs/multiplayer.md).
