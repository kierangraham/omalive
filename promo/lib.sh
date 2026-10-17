# Shared helpers for the promo recordings: a headless Hyprland output that
# never touches the visible desktop, and a frame recorder for a region of it.
# shellcheck shell=bash

promo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir=${PROMO_OUT:-$promo_root/artifacts/promo}
output=PROMO
workspace=omalive-promo
mkdir -p "$out_dir"

lua() { hyprctl eval "$1" | grep -iv '^ok$' >&2 || true; }

headless_up() {
  hyprctl output create headless "$output" >/dev/null
  lua "hl.monitor({ output = '$output', mode = '1920x1080@60', position = '-20000x-20000', scale = 1 })"
  lua "hl.workspace_rule({ workspace = 'name:$workspace', monitor = '$output', default = true })"
  sleep 0.5
}

headless_down() {
  hyprctl output remove "$output" >/dev/null 2>&1 || true
}

# Start a command on the headless workspace (never on the visible one).
launch() {
  lua "hl.exec_cmd('$1', { workspace = 'name:$workspace silent' })"
}

# The pid and geometry ("x,y wxh") of the first window of `class`.
window() {
  hyprctl clients -j | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["class"] == sys.argv[1] and c["workspace"]["name"] == sys.argv[2]:
        x, y = c["at"]; w, h = c["size"]
        print(c["pid"], f"{x},{y} {w}x{h}")
        break' "$1" "$workspace"
}

# record GEOMETRY OUT.mp4 — capture frames until stop_recording, stamped
# with the wall clock and resampled to a steady 30 fps.
record() {
  local geometry=$1 out=$2
  stop_file=$(mktemp -u /tmp/omalive-promo-stop.XXXXXX)
  {
    while [[ ! -e $stop_file ]]; do grim -t ppm -g "$geometry" - 2>/dev/null || break; done
  } | ffmpeg -hide_banner -loglevel error -y -f image2pipe -use_wallclock_as_timestamps 1 \
      -c:v ppm -i - -vf "fps=30,format=yuv420p" -c:v libx264 -preset fast -crf 12 "$out" &
  recorder=$!
  record_start=$(date +%s.%N)
}

stop_recording() {
  touch "$stop_file"
  wait "$recorder" 2>/dev/null || true
  rm -f "$stop_file"
}

# mark NAME — note when something happened, relative to the recording.
mark() {
  echo "$1 $(echo "$(date +%s.%N) - $record_start" | bc -l | cut -c1-6)" >> "$markers"
}
