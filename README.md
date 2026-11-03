# Motocross Madness 2 on Web (mcm2-web)

Play Motocross Madness 2 in your desktop browser, from your own copy of the game. It
uses WebAssembly and WebGPU, runs on your machine, and nothing is uploaded.

mcm2-web is a browser port of [openmcm2](https://github.com/crspytopgn/openmcm2), the
open-source Rust engine by [crspytopgn](https://github.com/crspytopgn). The engine, the
physics, the file formats and the menus are their work; this project adds the web port.
See [Credits](#credits).

> **This repository contains no game files.** The engine reads the tracks, models,
> textures, sounds and menus from an install of the game that you own, the way OpenMW does
> for Morrowind. Nothing of the game is hosted or shipped here: no data, no artwork, no
> code from the original game.

[Why](#why-a-web-version) · [Play](#play) · [How it works](#how-it-works) · [Build](#build) · [Deploy](#deploy-to-github-pages) · [Credits](#credits) · [Legal](#legal) · [Licence](#licence)

## Why a web version

Motocross Madness 2 was the first game I ever played on a PC, in the early 2000s. I loved
it as a kid, and I wanted to feel that nostalgia again without the hassle.

The hassle is real. The game isn't sold anywhere any more, and support for it ended long
ago. It was built for the Windows and DirectX of the early 2000s, and on current Windows
(10 and 11) it usually won't start, and when it does the graphics, sound or input are
broken. On Linux and macOS it doesn't run at all without extra compatibility software. Fixes and workarounds are
available online, but they mean downloading random files, software or code from the
internet, which is inconvenient and a risk.

So this needs nothing: no downloads, no installs, no compatibility fixes. Open the page,
choose your installed game folder, and play.

## Play

Click [here](https://rzrabbi.github.io/mcm2-web/) to open the site.
You need:

* a desktop browser with WebGPU: a recent Chrome or Edge (only Chrome has been tried);
* your own installed copy of Motocross Madness 2, the folder that contains `UI`, `RES`
  and `TERAFORM`.

Choose that folder, wait while it is read, press **Start game**, and create a profile the
first time. The site holds only the engine and the page, so your game files never leave
your computer and there is nothing to log in to.

Controls are the original's: the arrow keys to ride, W/S to lean, Z/X for stunts. Xbox and
PlayStation gamepads work too.

### Unsupported devices and browsers

The page checks your device and browser before it reads any game file, and shows what is
wrong instead of starting:

* **Phones and tablets** are not supported. Most mobile browsers lack WebGPU or run it too
  slowly for this engine, and there are no touch controls: the game needs a keyboard or
  gamepad. Use a desktop or laptop.
* **Browsers without WebGPU** (older versions, Safari, Linux Firefox) cannot draw the game.
  Use a recent Chrome or Edge, or Firefox on Windows.
* **WebGPU with no usable graphics** (graphics acceleration turned off, a blocked or old
  driver, a virtual machine without 3D) is reported with the setting to change.
* **Pages that are not HTTPS or localhost** are refused, because WebGPU needs a secure page.

## How it works

### The engine

The engine is written in Rust and rebuilds the game from the files of your own install: the
menus, bikes, riders and sounds from `UI` and `RES`, and the tracks from `TERAFORM`. It
runs the original's physics and race rules, draws the world and the menus with WebGPU, and
decodes the intro and credits movies itself.

### The web build

The same Rust code is compiled to WebAssembly and runs in the page. When you choose your
folder, its files are read into memory in the browser and never sent anywhere, and track
data loads when you open an event. The engine draws to a canvas with WebGPU, plays sound
through Web Audio, and keeps your profile, settings and records in the browser's own
storage.

## Build

Install [Rust](https://rustup.rs) and [Trunk](https://trunkrs.dev), then:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk

cd crates/mm2-viewer
trunk serve --port 8080        # development server at http://127.0.0.1:8080
trunk build --release          # optimised static files in dist/
```

To skip the folder picker while developing, serve your install locally and open the page
with `?game=`:

```sh
python tools/serve_game.py "/path/to/Motocross Madness 2" --port 8081
# then open http://127.0.0.1:8080/?game=http://127.0.0.1:8081
```

The same crate also builds as a desktop viewer for testing:
`cargo run --release -p mm2-viewer -- "/path/to/Motocross Madness 2"`.

This repository has no desktop launcher. If you want the desktop version with its installer
(setting up the game from a CD image, disc or install folder) and track downloads, use
[openmcm2](https://github.com/crspytopgn/openmcm2) itself.

## Deploy to GitHub Pages

The release build is plain static files, and WebGPU needs HTTPS, which Pages provides.

1. Set **Settings > Pages > Source** to **GitHub Actions**.
2. Push to `main`. [`pages.yml`](.github/workflows/pages.yml) builds the site with Trunk
   and publishes it.

The workflow builds with `--public-url /mcm2-web/`; change that if your repository has a
different name. Publish the engine and the page only, never the game's files.

## Credits

| Work | Author | Licence |
| --- | --- | --- |
| [openmcm2](https://github.com/crspytopgn/openmcm2) v0.2.1: the engine, physics, file formats and menus this port is built on | [crspytopgn](https://github.com/crspytopgn) and contributors | MIT or Apache-2.0 |
| Indeo 5 decoder (`crates/mm2-indeo`), a port of [FFmpeg](https://ffmpeg.org)'s | Maxim Poliakovski and the FFmpeg developers | LGPL 2.1 or later |
| Ground materials for the optional enhanced graphics, from [ambientCG](https://ambientcg.com) | ambientCG | CC0 |
| *Motocross Madness 2* | Rainbow Studios | © Microsoft |

To support the engine's author, you can
[sponsor crspytopgn on GitHub](https://github.com/sponsors/crspytopgn).

## Legal

mcm2-web is an independent fan project. It is not affiliated with, endorsed by or
sponsored by Microsoft or Rainbow Studios. Motocross Madness is a trademark of Microsoft
Corporation, and the game, its data and artwork are © Microsoft. This repository is a new
implementation with no code or data from the original game, and you need your own
legitimately obtained copy of Motocross Madness 2 to use it. Never commit game files,
assets extracted from them, or texture packs made from them.

## Licence

Licensed under either the [Apache License 2.0](LICENSE-APACHE) or the
[MIT licence](LICENSE-MIT), at your option, except `crates/mm2-indeo`, which is under the
[LGPL 2.1 or later](crates/mm2-indeo/COPYING.LGPLv2.1).
