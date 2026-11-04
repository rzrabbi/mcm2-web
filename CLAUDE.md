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
- **Keep the page self-contained:** no external scripts or services. The one exception on this
  branch is online multiplayer, which talks to a relay server the player runs (`mp.js` is the only
  code that does); nothing of the game goes to it.
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

python tools/serve_game.py "<game folder>" --port 8081  # then open :8080/?game=http://127.0.0.1:8081
python tools/relay/relay.py                           # the multiplayer relay (a page on localhost finds :2108)
python -m unittest discover -s tools/relay/tests      # the relay's tests (needs the websockets package)
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

## Known open issues

See ARCHITECTURE.md §8. In short: a failed track fetch leaves that event stuck until
reload; two quick event opens overlap their fetches; the loader path for the folder picker
(`TERAFORM` fix) still needs a browser test. Online multiplayer needs a relay you run (docs/multiplayer.md).
