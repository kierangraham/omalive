#!/usr/bin/env bash
# Screenshot the real app without touching the visible desktop: a headless
# Hyprland output is created, omalive (demo mode by default) opens silently
# on a workspace there, grim captures it, and the output is removed.
#
#   scripts/screenshot.sh OUT.png [WIDTHxHEIGHT] [SCALE] [-- omalive args]
set -euo pipefail
out=${1:?output png}
size=${2:-1180x820}
scale=${3:-2}
shift $(( $# >= 3 ? 3 : $# ))
[[ ${1:-} == -- ]] && shift
args=("$@")
[[ ${#args[@]} -eq 0 ]] && args=(--demo)
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary=${OMALIVE_BINARY:-$root/target/debug/omalive}
name=PBSHOT
width=${size%x*}
height=${size#*x}
cleanup() {
    [[ -n ${pid:-} ]] && kill "$pid" 2>/dev/null || true
    hyprctl output remove "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT
# Hyprland 0.56+ configures through Lua (hyprctl eval).
lua() { hyprctl eval "$1" | grep -iv '^ok$' >&2 || true; }
hyprctl output create headless "$name" >/dev/null
lua "hl.monitor({ output = '$name', mode = '$((width * scale))x$((height * scale))@60', position = '-20000x-20000', scale = $scale })"
lua "hl.workspace_rule({ workspace = 'name:omalive-shot', monitor = '$name', default = true })"
sleep 0.5
before=$(hyprctl clients -j | python3 -c 'import json,sys; print(" ".join(str(c["pid"]) for c in json.load(sys.stdin)))')
lua "hl.exec_cmd('$binary ${args[*]}', { workspace = 'name:omalive-shot silent' })"
for _ in $(seq 60); do
    pid=$(hyprctl clients -j | python3 -c '
import json,sys
before=set(sys.argv[1].split())
for c in json.load(sys.stdin):
    if c["class"]=="omalive" and str(c["pid"]) not in before: print(c["pid"]); break' "$before")
    [[ -n $pid ]] && break
    sleep 0.25
done
[[ -n ${pid:-} ]] || { echo "omalive window did not appear" >&2; exit 1; }
sleep "${OMALIVE_SHOT_DELAY:-2.5}"
if [[ -n ${OMALIVE_SHOT_KEYS:-} ]]; then
    for key in $OMALIVE_SHOT_KEYS; do
        lua "hl.dispatch(hl.dsp.send_shortcut({ mods = '', key = '$key', window = 'pid:$pid' }))"
        sleep 0.4
    done
    sleep "${OMALIVE_SHOT_SETTLE:-1.5}"
fi
grim -o "$name" "$out"
echo "$out"
