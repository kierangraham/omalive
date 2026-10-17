#!/usr/bin/env bash
# Record the bar widget against `omalive --demo`: on air, a change of look,
# mute and back. PROMO_LAYOUT=zoom (default) magnifies the widget in the
# bar; PROMO_LAYOUT=panel shows its quick panel. Writes bar-LAYOUT.mp4.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

binary=${OMALIVE_BINARY:-$promo_root/target/release/omalive}
picture=${PROMO_PICTURE:-/usr/share/omarchy/themes/last-horizon/backgrounds/1-eyes-wide.jpg}
omarchy_path=${OMARCHY_PATH:-/usr/share/omarchy}
socket=/tmp/omalive-promo-bar.sock
layout=${PROMO_LAYOUT:-zoom}
markers=$out_dir/bar-$layout.markers
: > "$markers"
scene=$(mktemp -d /tmp/omalive-promo-scene.XXXXXX)

cleanup() {
  [[ -n ${demo:-} ]] && kill "$demo" 2>/dev/null
  [[ -n ${pid:-} ]] && kill "$pid" 2>/dev/null
  headless_down
  rm -rf -- "$scene"
}
trap cleanup EXIT

# A Quickshell config: the shell's Commons and Ui, the plugin, and the
# in-window panel stand-in from the widget's render tests.
ln -s -- "$omarchy_path/shell/Commons" "$scene/Commons"
ln -s -- "$promo_root/shell" "$scene/plugin"
mkdir "$scene/Ui"
for file in "$omarchy_path"/shell/Ui/*; do ln -s -- "$file" "$scene/Ui/"; done
ln -sf -- "$promo_root/shell/tests/shim/KeyboardPanel.qml" "$scene/Ui/KeyboardPanel.qml"
cp -- "$promo_root/promo/bar-scene.qml" "$scene/shell.qml"

rm -f "$socket"
OMALIVE_SOCKET=$socket OMALIVE_DEMO_PICTURE=$picture "$binary" --demo --background >/dev/null 2>&1 &
demo=$!
headless_up
scale=1
[[ $layout == zoom ]] && scale=7.5
launch "env OMALIVE_BIN=$binary OMALIVE_SOCKET=$socket PROMO_LAYOUT=$layout QT_SCALE_FACTOR=$scale QT_QPA_PLATFORM=wayland quickshell -p $scene"
for _ in $(seq 80); do
  read -r pid geometry < <(window org.quickshell) || true
  [[ -n ${pid:-} ]] && break
  sleep 0.25
done
[[ -n ${pid:-} ]] || { echo "the scene did not open" >&2; exit 1; }
echo "scene $pid at $geometry"
send() { printf '%s\n' "$1" | OMALIVE_SOCKET=$socket "$binary" status --follow >/dev/null; }

# Fill the waveform history first.
sleep 6
record "$geometry" "$out_dir/bar-$layout.mp4"
mark air
sleep 5
mark look
send '{"cmd":"look","file":"Teal_and_orange.cube"}'
sleep 3.5
mark mute
send '{"cmd":"toggle-mute"}'
sleep 2.2
send '{"cmd":"toggle-mute"}'
mark unmute
sleep 3.5
send '{"cmd":"look","file":"Warm_film.cube"}'
sleep 2
mark end
stop_recording
cat "$markers"
ffprobe -v error -show_entries format=duration -of csv=p=0 "$out_dir/bar-$layout.mp4"
