#!/bin/sh
# Upscales an `mm2-tools textures` export 4x with Real-ESRGAN, using the
# upscaler inside Upscayl.app (or a separate realesrgan-ncnn-vulkan).
#
#   tools/upscale-textures.sh <export-dir> <pack-dir>
#
# Then run the viewer with `--hires <pack-dir>`. Environment:
#   UPSCALER  path to upscayl-bin or realesrgan-ncnn-vulkan
#   MODELS    folder holding the model .param/.bin files
#   MODEL     model name (default: the first of a few good ones found)
#   SCALE     2, 3 or 4 (default 4)
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 <export-dir> <pack-dir>" >&2
    exit 2
fi
in=$1
out=$2
scale=${SCALE:-4}

app=/Applications/Upscayl.app/Contents/Resources
if [ -z "${UPSCALER:-}" ]; then
    for c in "$app/bin/upscayl-bin" "$(command -v upscayl-bin || true)" \
        "$(command -v realesrgan-ncnn-vulkan || true)"; do
        if [ -n "$c" ] && [ -x "$c" ]; then
            UPSCALER=$c
            break
        fi
    done
fi
if [ -z "${UPSCALER:-}" ]; then
    echo "no upscaler found: install Upscayl, or set UPSCALER" >&2
    exit 1
fi
if [ -z "${MODELS:-}" ]; then
    MODELS=$app/models
    [ -d "$MODELS" ] || MODELS=$(dirname "$UPSCALER")/models
fi
if [ -z "${MODEL:-}" ]; then
    for m in realesrgan-x4plus upscayl-standard-4x ultrasharp-4x ultrasharp; do
        if [ -f "$MODELS/$m.param" ]; then
            MODEL=$m
            break
        fi
    done
fi
if [ -z "${MODEL:-}" ] || [ ! -f "$MODELS/$MODEL.param" ]; then
    echo "model ${MODEL:-?} not found in $MODELS; these are there:" >&2
    ls "$MODELS" | sed -n 's/\.param$//p' >&2
    exit 1
fi

echo "upscaling ${scale}x with $MODEL ($UPSCALER)"
for dir in models terrain skies images; do
    [ -d "$in/$dir" ] || continue
    mkdir -p "$out/$dir"
    echo "$dir: $(ls "$in/$dir" | wc -l | tr -d ' ') files"
    "$UPSCALER" -i "$in/$dir" -o "$out/$dir" -s "$scale" -m "$MODELS" -n "$MODEL" -f png
done
echo "done: run the viewer with --hires $out"
