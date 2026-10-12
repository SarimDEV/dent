#!/usr/bin/env bash
# Record the spike's demo scenes and turn them into X-ready clips and stills.
# Output: media/raw (masters), media/x (mp4 + posters + stills). media/ is gitignored.
# Needs ffmpeg and screen-recording permission for the terminal. Uses no agent usage.
set -euo pipefail
cd "$(dirname "$0")/.."
bin=target/debug/spike-gpui-chat
cargo build -q -p spike-gpui-chat
mkdir -p media/raw media/x

scenes=("stream paper" "stream carbon" "approval paper" "approval carbon" "scroll paper" "theme paper" "rest paper" "rest carbon")
for s in "${scenes[@]}"; do
  read -r name theme <<<"$s"
  flag=""; [ "$theme" = carbon ] && flag="--dark"
  out="media/raw/$name-$theme.mov"
  echo "recording $name ($theme)"
  $bin --demo "$name" --record "$out" $flag >/dev/null 2>&1
  sleep 1 # let ffmpeg finish writing
done

# Trim the recorder lead (1.5 s minus a beat), encode for X: H.264, yuv420p, faststart.
for mov in media/raw/*.mov; do
  base=$(basename "$mov" .mov)
  case "$base" in rest-*) continue ;; esac
  ffmpeg -y -v error -ss 1.2 -i "$mov" -vf "scale=1920:-2:flags=lanczos" \
    -c:v libx264 -crf 17 -preset slow -pix_fmt yuv420p -movflags +faststart "media/x/$base.mp4"
  # Close-up of the chat column, square-ish, for phones.
  ffmpeg -y -v error -ss 1.2 -i "$mov" -vf "crop=1360:1360:540:256,scale=1080:1080:flags=lanczos" \
    -c:v libx264 -crf 17 -preset slow -pix_fmt yuv420p -movflags +faststart "media/x/$base-closeup.mp4"
  # Poster from just before the end of the scene (the recording stops before the window closes).
  ffmpeg -y -v error -sseof -0.8 -i "$mov" -frames:v 1 -vf "scale=1920:-2:flags=lanczos" "media/x/$base-poster.jpg"
done

# Stills taken during the scenes.
for png in media/raw/*.png; do
  [ -e "$png" ] || continue
  ffmpeg -y -v error -i "$png" -vf "scale=2400:-2:flags=lanczos" -q:v 2 "media/x/$(basename "$png" .png).jpg"
done
ls -lh media/x
