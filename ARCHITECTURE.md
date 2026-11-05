# Architecture and project handbook — Motocross Madness 2 on Web (mcm2-web)

This file is the project handbook. It explains why the project exists, how it is
built, how it works, what has been done, what works and what does not, and the
legal position of every part. It is written for anyone picking the project up:
a new contributor, a reviewer, or an AI coding assistant.

Last updated: 2026-11-02.

---

## 1. What this project is

mcm2-web lets people play **Motocross Madness 2** (Rainbow Studios / Microsoft,
2000) in a desktop web browser, using **their own installed copy of the game**.

- The game engine is written in **Rust**, compiled to **WebAssembly**, and draws
  with **WebGPU**. Sound plays through Web Audio.
- The player opens the web page, chooses the folder where the game is installed,
  and plays. Nothing is installed, and nothing is uploaded: the files are read
  inside the browser and never leave the computer.
- The repository and the published site contain **no game files**: no data, no
  artwork, no code from the original game. The engine reads everything from the
  player's own install, in the same way OpenMW works with Morrowind.

The same code also builds as a **native desktop program** (Windows, Linux,
macOS) that reads the game folder from disk.

## 2. Why it was made (intent)

- Motocross Madness 2 is no longer sold anywhere, and support for it ended long
  ago.
- It was built for the Windows and DirectX of the early 2000s. On current
  Windows it often will not start, or runs with broken graphics, sound or input.
- Workarounds exist online (compatibility patches, wrappers, repacks), but they
  mean downloading and running random files, software or code from the
  internet, which is inconvenient and a security risk.
- The goal is a version that needs **nothing downloaded or installed**: open the
  page, choose the installed game folder, and play, with no compatibility
  settings, fixes or workarounds.

## 3. Origin: the openmcm2 engine

This project is a **browser port of [openmcm2](https://github.com/crspytopgn/openmcm2)**,
an open-source Rust re-implementation of Motocross Madness 2 by
[crspytopgn](https://github.com/crspytopgn) and contributors.

### 3.1 What comes from openmcm2

**Baseline:** openmcm2 **v0.2.1** (released 2026-10-29, commit `046c300`), the
latest upstream release when this port was made. The workspace version in
`Cargo.toml` (`0.2.1`) is upstream's number, not a version of this port. To take a
later upstream release, diff its tag against `046c300` and merge the engine crates;
the files most likely to conflict are this port's own: `platform.rs`, `index.html`
and the `web` module in `main.rs`.

The work began as a fresh clone of openmcm2. The following are **upstream work**,
kept under their original licence (MIT OR Apache-2.0), and should be credited to
openmcm2:

- the engine as a whole: the renderer, the race, the riders, the menus, the
  career, replays and the rest of `crates/mm2-viewer`;
- `crates/mm2-formats`: readers for the game's file formats;
- `crates/mm2-physics`: the bike physics, AI riders, camera, lap timing, stunts
  and the Pro Circuit career money, rebuilt from the original game;
- `crates/mm2-indeo`: the Indeo 5 video decoder (a port of FFmpeg's, see §9);
- `crates/mm2-tools`: command-line tools for inspecting the game's data;
- `assets/icon/`: the OpenMCM2 tyre logo, an original drawing;
- `crates/mm2-viewer/assets/ground/`: ambientCG ground materials (CC0);
- `tools/upscale-textures.sh` and the optional "enhanced graphics" and hi-res
  texture features.

The comments in the engine name addresses in the original executable (for
example `0x4b2bd0`). These are reverse-engineering references recorded by the
upstream author, showing which original routine each piece of code follows.
They are notes, not copied code.

### 3.2 About the upstream project

Facts from the upstream repository's own page (checked 2026-11-02):

- **What it is:** an open-source engine for Motocross Madness 2, rewritten from
  scratch in Rust. It runs natively on macOS (Metal), Windows (DX12/Vulkan) and
  Linux (Vulkan), rendering with `wgpu`. It ships no game data.
- **Front end:** the original's profiles, main menu, single-player events, Bike &
  Rider, options, the Pro Circuit career, replays, and network races between
  copies of the engine.
- **Events:** Supercross, Nationals, Baja and Enduro gate races, Stunt Quarry,
  Moto-Tag and ghost races.
- **Launcher (not in this repository):** upstream also has an `mm2-launcher`
  crate that sets the game up from a CD image, a disc or an install folder, and
  a "Get tracks" section that downloads player-made tracks from MDX Racing. This
  port has neither: the browser reads the install folder directly.
- **Controls:** per-profile keyboard mapping with the original's factory layout
  as the default; Xbox and PlayStation pads and flight sticks with rebindable
  buttons.
- **Optional extras (off by default):** `--enhanced` (a remastered look: sun
  shadows, HDR, ambient occlusion, widescreen race display, F3 compares) and
  `--hires <pack>` (high-resolution textures the user makes from their own copy).
  See §15.
- **Accuracy:** the player's bike runs the game's full physics model and the
  computer riders run the original's AI model. Riding is checked against
  recordings of the original game, and most comparison runs match on crash
  outcome, speed and pitch. Upstream lists these known differences: some jump
  landings, rough-ground behaviour and knife-edge object clips; computer riders
  crash somewhat more often and perform fewer tricks.
- **How it was written:** upstream states that its code and documentation were
  written by an AI coding agent (Claude Code) under one human fan's direction,
  with human testing and review welcomed.
- **Licence and legal handling:** dual-licensed Apache-2.0 or MIT; the
  `mm2-indeo` crate stays LGPL 2.1 or later; contributions are dual-licensed the
  same way. Like OpenMW, it reads assets from the user's own install and never
  downloads game data. It states it is not affiliated with Microsoft or Rainbow
  Studios.
- **Other upstream content not in this repository:** `site/` (the project web
  page), `RELEASE_NOTES.md`, the macOS app-bundle script, and the `mm2-launcher`
  crate. Upstream had 17 commits, 4 open pull requests, 6 stars and 2 forks when
  checked.
- **Support:** free, with optional sponsorship through GitHub Sponsors.

### 3.3 What this port adds

The web port is the work of this repository. In short:

- an in-memory file system and browser shims (`crates/mm2-viewer/src/platform.rs`);
- the wasm entry points and the asynchronous WebGPU start-up (the `web` module in
  `crates/mm2-viewer/src/main.rs`);
- the web page itself (`crates/mm2-viewer/index.html`): folder picker, file
  loading, saves and the Profiles panel (list, delete, zip backup and restore),
  sound unlocking, the custom pointer, the canvas sizing;
- a development file server (`serve-game` in `crates/mm2-tools/src/serve.rs`);
- free fonts that stand in for Arial (`crates/mm2-viewer/fonts/`);
- many fixes and features listed in §6.

## 4. Repository layout

```
.
├── Cargo.toml                 Workspace (version 0.2.1, edition 2021, MIT OR Apache-2.0)
├── Cargo.lock
├── README.md                  Public README (play, build, deploy, credits, legal)
├── ARCHITECTURE.md            This handbook
├── CLAUDE.md                  Short guide for AI coding assistants
├── LICENSE-MIT, LICENSE-APACHE
├── .github/workflows/
│   ├── ci.yml                 Tests + wasm check on every push / pull request
│   └── pages.yml              Builds the site with Trunk, publishes to GitHub Pages
├── assets/icon/               OpenMCM2 logo, app icon, .ico, social preview
├── crates/
│   ├── mm2-formats/           File format readers (library)
│   ├── mm2-physics/           Bike physics and race rules (library)
│   ├── mm2-indeo/             Indeo 5 video decoder (library, LGPL)
│   ├── mm2-tools/             Data inspection CLI and serve-game dev server (binaries)
│   └── mm2-viewer/            The game: engine + web page (binary)
│       ├── index.html         The web page (Trunk entry point)
│       ├── mp.js              Multiplayer tab and relay connection (copied into the build by Trunk)
│       ├── favicon.png
│       ├── fonts/             Liberation Sans (SIL OFL 1.1)
│       ├── assets/ground/     ambientCG materials (CC0)
│       ├── build.rs           Puts the icon on the Windows .exe
│       └── src/               About 50 modules, see §5.3
├── docs/multiplayer.md        Online multiplayer: how it plays, relay setup, troubleshooting
├── tools/
│   ├── relay/                 The multiplayer relay crate (mm2-relay), its Dockerfile, compose file and tests
│   └── upscale-textures.sh    Optional Real-ESRGAN texture upscaling
```

The reverse-engineering notes (physics, sound, Pro Circuit, enhanced graphics,
hi-res textures) and the Ghidra scripts are not part of this repository. They
live in the `docs/` folder of the upstream repository
([openmcm2](https://github.com/crspytopgn/openmcm2)); see §12.

The codebase is about 51,000 lines of Rust.

## 5. How it works

### 5.1 The game data

The game stores its data in two container formats, told apart by their first
four bytes:

- `RS2\0`: an uncompressed archive of named files.
- `FAOE`: an RS2 archive behind a simple byte cipher, used by most `.RES` packs
  and every `.ENV` track.

The install's important folders are `UI` (menus, as `.DAT` dialogs and their
pictures), `RES` (bikes, riders, sounds, engine samples) and `TERAFORM` (the
tracks, one folder per event type). Menu text comes from the string tables in
`UILANG.DLL` and `LANG.DLL`, so those two DLLs are read even though every other
`.dll` and `.exe` is skipped.

### 5.2 The web page (`crates/mm2-viewer/index.html`)

1. **Engine start.** Trunk builds the Rust crate to wasm and injects it. When it
   has loaded (`TrunkApplicationStarted`), the page enables the folder button.
2. **Choosing the folder.** The player either presses the button, which opens
   an `<input webkitdirectory>`, or drops the folder on the page (read with
   `webkitGetAsEntry`). Both work in every browser and give every file in the
   folder, `.dll` files included. Neither gives lasting access, so nothing is
   remembered and the folder is chosen again each visit. (Earlier versions used
   `showDirectoryPicker` and kept the handle; Chrome's picker hides `.dll`
   files, which meant asking for the two menu-text files separately.)
3. **Checking the folder.** The page confirms that `UI/MAINMENU.DAT` exists
   before using the folder. `UILANG.DLL` and `LANG.DLL`, which hold the menus'
   text, normally arrive with the folder. If a folder lacks them, the page asks
   for the two files on their own and keeps them in the browser's storage until
   "Use a different folder".
4. **Reading files.** Files the engine never reads (`.pdf .hlp .exe .dll .rtf
   .txt .icd .cur .016 .256 .sem`, except `UILANG.DLL`, `LANG.DLL` and
   `UI/PCNAMES.TXT`) are skipped to save memory. Everything else is handed to the engine with
   `add_file`. **Track files (`TERAFORM/…`) are not read up front**: they are
   registered with `add_remote_file`, and their bytes are only read when the
   player opens that event (see 5.4).
5. **Fonts.** The menus are set in Arial, which cannot be redistributed.
   Liberation Sans, which has the same letter widths, is served with the page and
   handed to the engine as `C:\Windows\Fonts\arial*.ttf` (`add_system_file`).
6. **Saves.** Everything the player saves (profiles, settings, Pro Circuit
   careers, records, replays and ghosts) lives under the engine's user folder
   `mm2-user`, and the page keeps it in IndexedDB (`mm2-saves`):
   - The `files` store maps each path to its bytes. The engine calls
     `mm2Persist(path, bytes)` and `mm2Forget(path, isFolder)` as it writes and
     deletes. A second store, `meta`, holds each file's last write time (ms), set
     by `mm2Persist`; saves from before it existed show "Updated unknown".
   - The page hands the files back with `add_saved_file` when "Start game" is
     pressed, not when the folder is read, so whatever the Profiles panel
     changed before that is what the game sees. It also asks the browser to keep
     the storage (`navigator.storage.persist`), which does not protect against
     the player clearing site data.
   - **Profiles panel (start page).** It needs only the browser's storage, not
     the engine. It lists each profile with its last update and size, and has
     Export, Delete, Export all and Import backup. A profile is
     `profile/<name>/` plus its Pro Circuit careers in `procircuit/<name>/`
     (the engine keeps them apart), and all of it is listed, exported, deleted
     and replaced together. Settings, records and replays are not tied to a
     profile and only travel in Export all.
   - **Backups** are zips laid out like the desktop save folder, with no
     `mm2-user/` prefix: `settings.ini`, `profile/<name>/<name>.prf`,
     `profile/<name>/control.ctl`, `procircuit/<name>/...`, `record/...`. They
     unpack into the desktop viewer's folder (`%APPDATA%\MM2` on Windows), and a
     zip of that folder imports here, so saves can move between the two builds.
     Names are `MCM2_profile_<name>_<date>_<time>.zip` and
     `MCM2_all-profiles_<date>_<time>.zip`. Files are stored uncompressed (the
     writer is a few lines, no library); the reader also takes deflated zips
     through `DecompressionStream`, checks each file's CRC, and ignores a single
     wrapper folder. It refuses encrypted and zip64 files, anything over 600 MB
     unpacked, and any path with `..`, a drive letter, a backslash or a control
     character.
   - **Guards against losing saves.** Import asks before it replaces anything
     already saved: a profile of the same name (compared without case, replaced
     as a whole with its careers) and any other file the backup has a copy of
     (settings, records, replays, so an old "Export all" cannot silently undo
     newer records). A file it brings replaces the stored one whatever its
     case, as the engine sees paths without case. Delete and Export read the
     profile's files at the moment of the click. While the game runs, its tab
     holds the Web Lock `mm2-game`, and the panel in another tab refuses to
     delete or import (the game's next save would undo it). One panel action
     runs at a time. If the saves cannot all be read at Start, the game is not
     started. Quitting from the main menu waits (up to 3 s) for writes still on
     their way. A failed write is reported in the console. An open tab of an
     older page that blocks the store's upgrade is reported on the page.
   - The start page's Graphics choices are in `localStorage`, not here, so they
     are not in a backup. The game's own Graphics options are in the profile
     and do travel.
7. **Start.** A "Start game" button starts the engine (`start_game`). The click
   matters: browsers keep sound off until the page has had a user gesture. The
   page wraps `AudioContext` so it can resume every context the audio library
   creates, on the first click or key and whenever the tab comes back into view.
8. **Pointer.** The game hides the system pointer and draws its own from
   `UI/CURSOR.TGA` (an arrow) and `UI/WAIT.TGA` (an hourglass while a track
   loads). The page decodes these 16-bit TGAs from the player's files and builds
   CSS cursors from them, so no game art is ever shipped.
9. **Canvas.** The canvas fills the window. It is drawn with at least 1,080
   pixel rows: on a smaller window the canvas is made larger and scaled down
   with a CSS transform, and on a larger one it uses the window's own pixels.
10. **Quit.** The X on the main menu reloads the page (the engine cannot start
    twice in one page). Saves are kept.

Debugging helpers on the page: `window.audioPeak()` returns the loudest recent
sample, to confirm sound is playing. The development loader, `?game=<url>`, loads
the files from `serve-game` (`crates/mm2-tools/src/serve.rs`) instead of the folder picker.

### 5.3 The engine (`crates/mm2-viewer/src`)

The main areas, grouped (one module per file):

- **Start-up and loop:** `main.rs` (window, event loop, race loading, the `web`
  module with the wasm entry points), `platform.rs` (desktop/browser
  differences).
- **Menus:** `frontend.rs` (screen flow and the main menu), `ui.rs` (the game's
  `.DAT` dialog system: controls, pull-downs, lists, text, tooltips),
  `sprites.rs` (2D drawing and the wide-window layouts), `screens/` (options,
  profiles, records, replays, credits, intro, Pro Circuit, multiplayer, key
  bindings).
- **Racing:** `race.rs`, `ride.rs`, `riding.rs`, `gates.rs`, `countdown.rs`,
  `finish.rs`, `podium.rs`, `hud.rs`, `tag.rs` (Moto-Tag), `replay.rs`,
  `procircuit.rs` (career rules), `roster.rs` (bikes and outfits),
  `profile.rs` (profile files).
- **Rendering:** `renderer.rs`, `objects.rs`, `ground.rs`, `sky.rs`,
  `particles.rs`, `dust.rs`, `rain.rs`, `ruts.rs`, `grass.rs`, `crowd.rs`,
  `scenery.rs`, `traffic.rs`, `morph.rs`, `post.rs`, `enhance.rs`,
  `graphics.rs`, `materials.rs`, `hires.rs`, `cull.rs`, `preview.rs`, and the
  WGSL shaders beside them.
- **Sound and video:** `audio.rs`, `sound.rs`, `reverb.rs`, `cues.rs`,
  `movie.rs`.
- **Input:** `input.rs`, `pad.rs` (gamepads through gilrs).
- **Network:** `net.rs` (the port's own LAN/TCP-IP play, replacing DirectPlay;
  desktop only for now).
- **Windows only:** `winwatch.rs` (logs windows that appear over the game).

### 5.4 Desktop and browser differences (`platform.rs`)

On the desktop each part of `platform` is a thin pass-through to `std`. In the
browser:

- `fs` is an **in-memory file tree** (case-insensitive) that the page fills. A
  file can be present but not loaded yet (a "remote" track file).
- `ensure_folder(dir)` reports whether an event's files are loaded. If they are
  not, it asks the page once (`mm2Fetch`), frees the previously opened event's
  track data to save memory, and returns `false`; the caller tries again on a
  later frame.
- Writes under the user folder (`mm2-user`) are forwarded to the page's saves.
- There are no threads. Track loading is an `async` task that yields to the
  browser between steps, so the page keeps painting; on the desktop the same
  task runs on a thread.
- `FILL_MENUS` and the wide-layout code make the 640×480 menus fill any window
  shape without distorting text and icons.
- `COMING_SOON` (in `frontend.rs`) greys out multiplayer in the browser. It is off on the
  `online-multiplayer` branch, where the network session runs over a WebSocket relay (see §7).

### 5.5 Physics and rules (`crates/mm2-physics`)

Units are feet, seconds and pounds, and coordinates follow the game (x right,
y up, z forward, left-handed). The crate models the player's bike (a hybrid of
the game's full vehicle model), the simpler computer-rider bike, AI line
following, the race camera's views, lap counting and positions, stunt scoring,
scripted traffic, crashes, and the Pro Circuit money and points.

### 5.6 Video (`crates/mm2-indeo`)

The intro, credits and main-menu clips are AVI files in Indeo Video Interactive 5
(`IV50`). This crate decodes them in Rust (no browser codec needed). Only what
Indeo 5 uses is implemented: no Indeo 4 and no B-frames.

## 6. What has been done in this port (progress)

All dated 2026-11-02 unless noted.

**Getting it running**
- The engine compiles to wasm and runs in Chrome with WebGPU.
- Fixed: WebGPU canvases have no sRGB format, so the surface is configured with
  an sRGB view.
- Fixed: menu text was missing because `UILANG.DLL` and `LANG.DLL` were filtered
  out with the other DLLs.
- Free fonts (Liberation Sans) replace Arial.

**Loading**
- Tracks load on demand per event, and other events' data is freed.
- Track loading no longer freezes the page: it is asynchronous and yields to the
  browser.
- The game's own loading bar is drawn and visibly fills.
- The folder handle was remembered between visits (replaced by the folder input and drag and drop, §5.2).

**Sound, video and input**
- Sound through Web Audio (cpal's `wasm-bindgen` backend), unlocked on the first
  gesture and resumed when the tab returns.
- An engine voice dropped by the mixer is recreated.
- Intro, main-menu and credits movies play (Indeo 5 decoded in Rust).
- Gamepads work in the browser (gilrs over `navigator.getGamepads`); tested with
  a simulated Xbox pad.

**Saves and records**
- Profiles, settings and records persist in IndexedDB across reloads.
- The start page can list, delete, export and import profiles as a zip in the
  desktop save-folder layout (see 5.2, item 6).
- Track records are now saved at the end of a race. The port previously had the
  High Scores screen but nothing ever added a record.

**Graphics and layout**
- Menus fill the whole window at any shape. Wide and tall windows stretch the
  background art while text, icons and panels keep their shape.
- The canvas is drawn with at least 1,080 rows.
- The graphics are chosen on the start page, as the desktop chooses them on its
  command line: Original (the game's own drawing, as without `--enhanced`) or
  Enhanced, with every upgrade, preset, hour, weather and slider `--enhanced=`
  takes. The page builds that list and the engine reads it with the same parser
  (`Enhanced::parse`), so both builds draw alike; the page shows the matching
  desktop flag. F3 compares the two looks in a race, and Options → Graphics
  works as on the desktop. WebGPU only draws 1× or 4× anti-aliasing.
- Fixed: pink fringes on menu art edges (magenta key colour bleeding through
  smoothing).
- Fixed: lag the first time a pull-down opens, and two pull-downs overlapping.

**Release build**
- `trunk build --release` runs wasm-opt: the wasm shrinks from about 25 MB to
  14 MB, and a track loads in about 1 to 3 seconds instead of 15 to 60 seconds.

**Final clean-up before publishing**
- `.gitignore` covers build output (`target/`, `dist/`), local tool settings
  (`.claude/`, `.vscode/`, `.idea/`), `.env`, OS files, a local `docs/re/` folder
  and every game file format.
- Source comments no longer point to a local `docs/` folder, so nothing in the
  repository refers to a file it does not contain.
- Multiplayer shows "COMING SOON" in the browser.
- Fixed: deleting a save folder from browser storage also deleted a separate
  save whose name was the folder's name followed by `0` (inclusive key range).
- Fixed: a folder that is not the game could be remembered and then reloaded,
  with an error, on every later visit.
- Fixed: with the folder picker, every track was read into memory at start-up,
  because the page tested for `terraform/` while the game's folder is
  `TERAFORM`. Tracks now load per event, as they already did through the
  development server. This path still needs a test in the browser (open several
  events in turn).

## 7. Status: what works and what does not

**Verified working in Chrome (desktop, NVIDIA GPU)**
- Main menu, profiles, Options (all six tabs), High Scores, Credits, Replays list.
- Races: Supercross, Baja, Enduro, Nationals, Stunts; Pro Circuit (new career,
  race, results, next race, the career persists).
- Sound in races, the engine sound, the intro and credits movies.
- Gamepad (simulated), saves and records across reloads.
- The Profiles panel with the real game files: a real profile (with its career)
  exported, deleted, and imported again came back as "Current Profile" in the
  game. Also tested on the page alone: list, delete, replace on import, bad
  paths refused, upgrade from the first saves database.
- Frame pacing in a release build: about 6.4 ms on average, 12 ms at the 99th
  percentile, at 1920×855 with maximum settings.

**Not working or not available**
- **Multiplayer in the browser:** the port's network code uses sockets, which
  browsers do not offer. On `main` the Multiplayer pull-down shows "COMING SOON" and its
  TCP/IP button is greyed out; multiplayer works in the desktop build. The
  `online-multiplayer` branch runs the same session over a WebSocket relay that the players
  run (see [docs/multiplayer.md](docs/multiplayer.md)); it keeps `main`'s self-contained design
  by living on a branch of its own.
- **Moto-Tag:** multiplayer only, so unavailable in the browser.
- **Random gates (Baja):** the option shows but does nothing, in the desktop
  build too. The original's random gate generation has not been
  reverse-engineered yet.
- **Help:** the original uses WinHelp `.HLP` files, which cannot be shown.
- **Mobile and tablets:** not supported. The game needs a desktop browser with
  WebGPU, a folder picker and a keyboard or gamepad.
- **Browsers:** only Chrome has been tested. Edge should behave the same.
  Firefox and Safari need WebGPU support and have not been tried.

**Not yet verified**
- Saving a replay or ghost; the 5- and 10-lap record tables; stunt records.
- Moving a save zip between this build and the desktop viewer (the layouts
  match by the code, but neither direction has been tried), and zips made by
  Windows Explorer or 7-Zip. The panel in browsers other than Chrome.
- The automatic resolution step-down on a genuinely slow GPU.
- A physical gamepad.
- Some screens after the wide-layout change (Pro Circuit, Race Info, Profiles,
  some Options tabs), and the loading screens of every event.
- The keyboard controls stated in the README ("arrow keys to ride, W/S to lean,
  Z/X for stunts"). The bindings come from the game's own defaults, and the
  statement has not been checked against them.

## 8. Known issues and follow-ups

These were found in the review on 2026-11-02:

1. **Fixed, untested in the browser: a failed track read left the event stuck.**
   The page now shows the error, and three seconds later calls `fetch_failed`
   (`main.rs`), which clears the folder's entry in `asked` (`platform.rs`) so the
   game asks again for the files still missing. It keeps retrying every three
   seconds while the files cannot be read.
2. **Fixed, untested in the browser: opening two events quickly overlapped their
   fetches.** `mm2Fetch` keeps the latest request in `trackLoad`; an earlier fetch
   stops adding files, and leaves the label alone, once it has been replaced.
3. **Unused code in `index.html`:** the "hold Esc to leave full screen" handler and
   the `fullscreenchange` listener do nothing, because the page never enters full
   screen.
4. **Spare number:** the port has no version number of its own yet; the crates carry
   upstream's `0.2.1`.
5. **Compiler warnings:** the wasm build reports 36 unused-code warnings
   (harmless, predating the clean-up).
6. **Tooling:** `clippy` and `rustfmt` are not installed on the development
   machine, so neither has been run.
7. **Release profile:** the workspace has no `[profile.release]` tuning (LTO,
   `codegen-units = 1`) yet; it might shrink and speed up the wasm further.
8. **Random gates** and **browser multiplayer** (see §7) remain the two missing
   game features on `main`; the `online-multiplayer` branch has the second.

## 9. Licences and credits

| Part | Author | Licence |
| --- | --- | --- |
| This repository's code (engine and port), except `mm2-indeo` | openmcm2 contributors and mcm2-web contributors | MIT OR Apache-2.0, at your option |
| `crates/mm2-indeo`, a port of FFmpeg's Indeo 5 decoder (`indeo5.c`, `ivi.c`, `ivi_dsp.c`) | Maxim Poliakovski and the FFmpeg developers | LGPL 2.1 or later (`crates/mm2-indeo/COPYING.LGPLv2.1`) |
| Liberation Sans 2.1.5 (`crates/mm2-viewer/fonts/`) | Google, Red Hat | SIL Open Font License 1.1 (`LICENSE-Liberation`) |
| Ground and object materials (`crates/mm2-viewer/assets/ground/`): Ground048, Ground103, Grass004, Rock030, Ground080, Wood049, Metal054B, Concrete035, ThatchedRoof001A | ambientCG | CC0 1.0 (public domain) |
| OpenMCM2 logo (`assets/icon/`), lettering in Rubik Bold (outlined) | openmcm2 | with the project; Rubik is SIL OFL |
| *Motocross Madness 2* (game, data, artwork) | Rainbow Studios | © Microsoft; trademark of Microsoft Corporation |

`LICENSE-MIT` names "the OpenMCM2 contributors" and "the mcm2-web contributors".
The engine's author can be supported at
[github.com/sponsors/crspytopgn](https://github.com/sponsors/crspytopgn).

**Legal position.** mcm2-web is an independent fan project, not affiliated with,
endorsed by or sponsored by Microsoft or Rainbow Studios. It is a new
implementation containing no code or data from the original game, and players
need their own legitimately obtained copy. The published site holds only the
engine and the page.

**Rules for contributors:**
- Never commit game files, anything extracted from them, or texture packs made
  from them. `.gitignore` blocks the game's formats (`*.RES *.ENV *.TEX *.DAT
  *.EXE *.DLL *.AVI *.WAV *.vcr *.gho`) and the `/GAME`, `/extracted` and
  `/textures-hires` folders.
- Never ship game art in the page. Anything drawn from the game (such as the
  pointer) must be built at run time from the player's own files.
- Keep the LGPL boundary: changes inside `crates/mm2-indeo` stay under the LGPL.

## 10. Building, running and deploying

**Prerequisites:** [Rust](https://rustup.rs) (stable) and
[Trunk](https://trunkrs.dev).

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk
```

**Browser build** (from `crates/mm2-viewer`):

```sh
trunk serve --port 8080        # development server at http://127.0.0.1:8080
trunk build --release          # optimised static files in dist/ (runs wasm-opt)
```

The dev build is unoptimised (about 25 MB of wasm, slow track loads). Use the
release build to judge speed.

**Skipping the folder picker while developing:**

```sh
cargo run --release --bin serve-game -- "/path/to/Motocross Madness 2" --port 8081
# then open http://127.0.0.1:8080/?game=http://127.0.0.1:8081
```

**Desktop build:**

```sh
cargo run --release -p mm2-viewer -- "/path/to/Motocross Madness 2"
```

It takes other arguments for development (open a single track, take a
screenshot, enhanced graphics, hi-res textures, frame-rate display); see the
comment at the top of `crates/mm2-viewer/src/main.rs`. On Linux it needs
`libasound2-dev` and `libudev-dev`.

**Tests:**

```sh
cargo test --release --workspace
```

There are 164 tests. The tests that need the real game (`mm2-formats/tests/game_data.rs`,
`mm2-physics/tests/real_track.rs`) are skipped unless `MM2_DATA` points at the
game's folder, so CI never needs game files.

**Check the browser build without Trunk:**

```sh
cargo check --release --target wasm32-unknown-unknown -p mm2-viewer \
  --no-default-features --features audio,gamepad
```

**Continuous integration** (`.github/workflows/ci.yml`): on every push to `main`
and every pull request, runs the tests on Ubuntu and checks the wasm build.

**Deployment** (`.github/workflows/pages.yml`): on every push to `main`, builds
with `trunk build --release --public-url /mcm2-web/` and publishes `dist/` to
GitHub Pages. Set **Settings → Pages → Source** to **GitHub Actions**. The
repository must be named `mcm2-web`, or the `--public-url` changed to match. WebGPU
needs HTTPS, which Pages provides.

**Reverse-engineering notes:** the analysis scripts (`docs/re/`) are not in this
repository (a local copy, if present, is excluded by `.gitignore`). Find them in the
upstream repository's `docs/re/` folder.

## 11. Conventions for working on the code

- **Comment style:** comments are plain sentences explaining what the original
  game does and where (with its address), and why the code follows it. Match
  that style and density.
- **Platform differences** go through `platform.rs` or a `cfg!(target_arch =
  "wasm32")` constant, so the desktop build stays unchanged unless a change is
  meant for both.
- **Verify in the browser** after changes to menus or rendering. The menu layout
  code is anchored by control rectangles, so untested screens can show odd
  alignments.
- **Keep the page self-contained:** no external scripts or services. The page
  must keep working with nothing but the published files and the player's game
  folder.

---

## 12. Reference: how the original game was understood

The engine is a re-implementation, so almost everything in it follows the
original program, `Mcm2.exe` (codename "krusty2"). The upstream author took the
original apart with the **Ghidra** decompiler (the executable is unprotected) and
wrote notes about it, published in the `docs/` folder of the upstream repository
(`physics.md`, `sound.md`, `procircuit.md`, `enhanced-graphics.md`,
`hires-textures.md`). Those notes are the map between the original's code and
this engine, and the engine's comments cite the original by address (the
executable's image base is `0x400000`). Sections 12 to 17 below summarise them.

- **Evidence labels.** The notes mark each finding **[solid]** (read carefully),
  **[likely]** (structure clear, meaning inferred) or **[open]** (not decoded).
- **Checked against recordings.** The engine's output is compared with captures
  of the original running (positions, speeds, crash outcomes, and the engine
  sound), so differences are measured rather than guessed.
- **Reproducing the analysis** (upstream `docs/re/`): Ghidra 12.1.4 headless with Java 21,
  running `MkFuncs.java`, `ParamId.java`, `CommitParams.java`, `ApplyClass.java`
  and `DumpAll.java`, then `show.py` and `cg.py` to read the output and
  `colparse.py` to parse collision files. The scripts need the original
  executable, which is **not** in this repository.
- **Constraint for contributors.** Describe findings in your own words and cite
  addresses. Never paste decompiled code, extracted data or artwork into the
  repository.

## 13. Reference: the original's physics and rules

The original's class chain is `SoultreePhysicsObject` (rigid body, fixed-step
loop) → `Vehicle` (tyres, shocks, drivetrain, input) → `Bike` (bike forces,
crashes, AI solver) → `KrustyBike` (game glue, controls, rider animation).

- **Two physics paths.** The player's bike uses the full vehicle model: a rigid
  body, two tyres, inline and rotating shocks, and a drivetrain. Computer riders
  use a cheaper two-point model (a "dumbbell" with wheel points 3.5 ft ahead of
  and behind the centre). A crashed bike goes back to the full model.
- **Frame rate.** The original runs the computer riders' model once per frame
  with the frame's length, so it only behaves as designed at the recorded
  frame rate (0.028 s). The engine runs the computer riders' controller and
  model in 0.028 s frames and draws them between frames; a crashed computer
  rider and the player's bike take whole 0.02 s steps.
- **Units.** Feet, seconds and pounds. Gravity is 32 ft/s², the wheelbase 7 ft,
  and the world is left-handed (x right, y up, z forward).
- **Collisions.** `.col` files (248 of them, all parsed) hold a body or a set of
  parts, each with a triangle tree for surfaces and a point tree for contact
  points. Terrain height and normal come from the `.trn` quadtree, where each
  cell is two triangles. Per-zone friction, drag and traction come from the
  scene's `[TerrainZoneN]` sections.
- **Other covered areas:** drivetrain and gears; input to physics; crash
  triggers and the ragdoll "Wrecker"; the in-race display; lap counting, gates,
  positions and times; stunts, tricks, points and prize money; the Pro Circuit
  career's money and points; the race camera's views; Stunt Quarry sessions and
  scripted traffic.
- **Known differences** (from the open items): the first-bounce behaviour of
  some head-on wall hits and some crest and jump landings still differ from the
  recordings; whether Baja rides entirely on the computer-rider model is likely
  but unconfirmed; one in-air action is unmapped; the Wrecker's push direction
  is not fully mapped.

## 14. Reference: sound in the original, and in the engine

(Code: `crates/mm2-viewer/src/sound.rs`, `reverb.rs`, `cues.rs`, `audio.rs`.)

- The original used DirectSound. Race sounds (except the crowd and the chimes)
  are 3D buffers. The listener is the camera, moving with the followed bike,
  with a distance factor of 0.3048 (the game works in feet), rolloff 1 and Doppler
  factor 1. Sounds are silent beyond their maximum distance (engines and wrecks
  500 ft, the hard landing 10 to 500 ft).
- **Engines.** Every bike's state machine runs each frame, but only the four
  bikes nearest the camera get a streaming buffer. Samples are chosen by engine
  class (125, 250, 400 cc) and by speed band. Which sample of a list plays is
  random, as in the original.
- **Effects.** Hard landing, wrecks, falls, the waypoint chime, the Moto-Tag "IT"
  sound, the stunt bonus sound, and crowd loops and applause.
- **Scene sounds.** 299 emitters across the tracks (windmills, trains, cars, oil
  wells and more) with their own distances, looping and trigger chances.
- **Music.** None in a race: the original opens the CD device only to stop it.
  The menus' sound comes from their movies.
- **Not ported:** EAX environments, the original's sound hotkeys, and
  DirectSound's software 3D pan law (the engine pans by the side the sound is
  on).
- **Verification:** the engine's engine-note output was compared with a capture
  of the original and matches the sequence and levels within about 1 dB.

## 15. Reference: the optional graphics features

Everything here is the engine's own, not the original's, and changes only how
the game is drawn: the physics, camera, clock and settings are identical.

**Enhanced graphics (`--enhanced`).** Off by default, and the drawing is then
pixel for pixel the original's. In the browser the start page's Graphics panel
gives the same choices (see §6).

- Quality presets `low`, `medium`, `high` (default) and `ultra`; and a
  `resolution=` cap on the pixels the 3D scene is drawn with (menus and the race
  display stay sharp).
- Upgrades, each switchable: anti-aliasing (MSAA), per-pixel lighting, sun
  shadows (three 2048² cascades to 520 ft), longer draw distance with haze, HDR
  with bloom (tone mapped), ambient occlusion, sky glow, a colour grade, photo
  ground detail (CC0 materials from `assets/ground`), smooth hills (finer ground
  near the camera), trackside props and 3D crowds, a real sky with clouds,
  time of day and weather (overcast, rain), tyre ruts, dust and spray, glossy
  bikes and riders, mud, light shafts, a replay lens (depth of field and motion
  blur, replays only), a widescreen race display, and 16× anisotropic filtering.
- **F3** compares enhanced and original drawing during a race, **F2** shows the
  frame rate and **Alt+Enter** toggles full screen.
- The code is in `graphics.rs`, `enhance.rs`, `post.rs`, `materials.rs`,
  `grass.rs`, `ruts.rs`, `dust.rs`, `rain.rs`, `smooth.rs`, `scenery.rs`,
  `crowd.rs` and the `.wgsl` shaders. The original's shaders and pipelines are
  untouched; enhanced pipelines are only made when asked for.
- Nothing touches the physics: a scripted ride gives byte-identical results with
  and without the upgrades.

**High-resolution texture packs (`--hires <dir>`).** Optional and desktop-first.
`mm2-tools textures <GAME> <out>` exports the game's textures as PNG (513 model
textures, 2,584 terrain textures, 70 sky faces, 772 menu images). The player
upscales them with Real-ESRGAN (for example through Upscayl, or
`tools/upscale-textures.sh`), and the engine uses any texture it finds in the
pack and falls back to the original for the rest. Packs are made from the
player's own game data and **must never be committed or published**. Menu art
(`images/`) is exported but not used yet.

## 16. Reference: the Pro Circuit career and the data files it reads

The career runs four series in order (Enduro, Baja, Nationals, Supercross) across
three levels, with a class (125, 250 or Open), a difficulty (Easy, Normal, Hard),
entry fees, prize shares by place, and caps for repairs, medical bills and stunt
money. Computer riders get random names and rides that change with each level.

- Data comes from `UI/UIRES.RES` (`PCTables.pb`, `[ProCircuit]`), `PCSched.pb`
  (tracks per series, laps, bonus track), `UI/PCNames.txt` (rival names) and the
  availability levels in `bikes.pb` and `riders.pb`.
- The circuit is saved whole (4,645 bytes) with each rider's last-race and
  season totals. Each profile's careers are kept in `procircuit/<profile>/`
  (`Career1.txt`, ...) in the user folder, apart from `profile/<profile>/`;
  the web page's Profiles panel treats the two as one profile.
- The rules are in `procircuit.rs` and the screens in `screens/pro.rs`.

## 17. Reference: command-line options (desktop)

`mm2-viewer` accepts, among others: a game folder (opens the menus), a `.ENV`
track file (opens that track), `--laps`, `--riders`, `--screenshot <png>` with
`--size`, `--camera` and `--time`, `--ride <seconds>` with `--steer` and
`--front` (a scripted ride), `--hires <dir>`, `--enhanced[=options]`,
`--fullscreen`, `--no-vsync` and `--show-fps`. The full list is in the comment at
the top of `crates/mm2-viewer/src/main.rs`. Free camera: hold the right mouse
button to look, WASD to fly, Q/E or Space/Ctrl to go down or up, Shift for speed,
Tab to switch to it, R to restart, P to let the computer ride for you.

`mm2-tools` inspects and unpacks data files and exports textures
(`mm2-tools textures <GAME> <out>`).
