# High-res texture packs

The engine can swap the game's textures for AI-upscaled ones. This is
optional: by default the engine draws the game's own textures, and it
only uses a pack when you pass `--hires <dir>`.

Packs are made from the game data, so keep them out of any public repo.

## 1. Export the textures

```sh
cargo run --release -p mm2-tools -- textures ../GAME ~/mm2-textures
```

This takes about two minutes. It writes four flat folders of PNGs:

| Folder | What | Count | Used by the engine |
| --- | --- | --- | --- |
| `models/` | every model texture: bikes, riders, stadiums, banners, trees, cars | 513 | yes |
| `terrain/` | every track's ground textures (256×256) | 2,584 | yes |
| `skies/` | the 14 sky boxes, five 512×512 faces each (four sides, then the top; the sides' lower half is clear) | 70 | yes |
| `images/` | menu art, loading screens, fonts, HUD (radar, particles) | 772 | not yet: menus come later |

Not exported: `RES/MODELTGA.RES`, which only holds 8-bit palette copies
of the model textures for old graphics cards.

The magenta that marks see-through pixels is turned into real
transparency, and those pixels take the colour of their neighbours, so the
upscaler doesn't smear pink into the edges of fences, crowds and trees.

## 2. Upscale them

### With the Upscayl app

[Upscayl](https://upscayl.org) is a free Mac app that runs Real-ESRGAN on
the GPU. Use **Batch Upscayl** on each folder in turn (`models`, `terrain`,
`skies`, `images`), with:

- Scale **4x**, and **PNG** as the output format.
- An output folder of the same name inside your pack, for example
  `~/mm2-hires/models`.
- Model: try "Real-ESRGAN x4plus" (called "Upscayl Standard" in newer
  versions) first. "UltraSharp" is also good for game art. Upscale a few
  textures with each and compare before doing the whole batch.

Upscayl adds `_upscayl_4x_<model>` to file names. The engine ignores that
part, so there is no need to rename anything.

### With the script

`tools/upscale-textures.sh` runs the same upscaler, from inside Upscayl.app,
on all four folders in one go:

```sh
tools/upscale-textures.sh ~/mm2-textures ~/mm2-hires
MODEL=ultrasharp-4x tools/upscale-textures.sh ~/mm2-textures ~/mm2-hires
```

It also works with a separate `realesrgan-ncnn-vulkan` (set `UPSCALER` and
`MODELS`).

## 3. Use the pack

```sh
cargo run --release -p mm2-viewer -- ../GAME/TERAFORM/SX/SX01.ENV --hires ~/mm2-hires
```

The viewer says how many high-res textures it found and how many the track
uses. Any texture without an image in the pack, or with an image that
can't be used (not square, not a power of two, or smaller than the
original), stays as the original; unusable images get a warning. So a
pack can be partial: for example, only the bikes and riders.

You can also edit textures by hand: change the exported PNG, keep its name
and size (or a power-of-two multiple of it), and put it in the pack.

Memory: 4x makes each texture 16 times bigger. A track's terrain then
needs roughly 200 to 600 MB of graphics memory, which is fine on Apple
silicon.

## Details

`mm2_formats::hires` holds the conventions both sides share:

- Names: `<texture>.png` for model textures (`barrel.tga.png`),
  `<set>-<texture>.png` for the four stadium sets in `RES/*.TEX`, which
  reuse each other's names (`euro-wall.tga.png`), and
  `<track>-terrain-<index>.png` for terrain (`sx01-terrain-007.png`),
  and `sky-<sky>-<face>.png` for sky faces (`sky-terra01-4.png`). Case doesn't matter, and the pack can use any subfolders.
- Rows: the game stores rows bottom-up; the PNGs are top-down, so they
  look the right way up.
- Transparency: for magenta-keyed textures, alpha below 128 is
  transparent. If the upscaler dropped the alpha channel, the engine takes
  it from the original texture, scaled up. ARGB4444 textures keep their
  own alpha the same way. Sky faces always take their alpha from the
  original.
- Mips are rebuilt from the upscaled image.
