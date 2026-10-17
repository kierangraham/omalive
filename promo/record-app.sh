#!/usr/bin/env bash
# Record the real omalive window in demo mode on a headless output: the
# voice page (walking the chain, muting), then the camera page cycling
# through looks. Writes app.mp4 and app.markers to artifacts/promo/.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

binary=${OMALIVE_BINARY:-$promo_root/target/release/omalive}
picture=${PROMO_PICTURE:-/usr/share/omarchy/themes/last-horizon/backgrounds/1-eyes-wide.jpg}
socket=/tmp/omalive-promo.sock
markers=$out_dir/app.markers
: > "$markers"

cleanup() {
  [[ -n ${pid:-} ]] && kill "$pid" 2>/dev/null
  headless_down
}
trap cleanup EXIT
headless_up
rm -f "$socket"
launch "env OMALIVE_SOCKET=$socket OMALIVE_DEMO_PICTURE=$picture $binary --demo"
for _ in $(seq 60); do
  read -r pid geometry < <(window omalive) || true
  [[ -n ${pid:-} ]] && break
  sleep 0.25
done
[[ -n ${pid:-} ]] || { echo "omalive did not open" >&2; exit 1; }
echo "window $pid at $geometry"

key() { lua "hl.dispatch(hl.dsp.send_shortcut({ mods = '', key = '$1', window = 'pid:$pid' }))"; }
send() { printf '%s\n' "$1" | OMALIVE_SOCKET=$socket "$binary" status --follow >/dev/null; }

# Let the spectrum and waveform fill up before rolling.
sleep 4
record "$geometry" "$out_dir/app.mp4"
mark voice
sleep 3.5
mark chain
for _ in 1 2 3 4 5; do key bracketright; sleep 1.2; done
mark mute
key m; sleep 1.6; key m; sleep 1.4
mark camera
key 2
sleep 3
for look in Teal_and_orange Mono Clean_day Warm_film; do
  mark "look-$look"
  send "{\"cmd\":\"look\",\"file\":\"$look.cube\"}"
  sleep 2.6
done
mark original
send '{"cmd":"bypass"}'; sleep 2.2
send '{"cmd":"bypass"}'; sleep 2
mark end
stop_recording
cat "$markers"
ffprobe -v error -show_entries format=duration -of csv=p=0 "$out_dir/app.mp4"
