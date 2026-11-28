# What the macOS runner's scripts share (sourced by showcase.sh, verify.sh, dmg-check.sh): an
# MDM of its own (its own data folder, MDM_DATA_DIR, and download folder), the lab server, the
# windows of MDM's process found by id (mactool), screenshots by window (screencapture -l), and
# quitting MDM as a person does (/exit), timed. macOS's bash is 3.2 and has no `timeout`: perl's
# alarm stands in.
#
# The caller sets MDM_EXE (the program inside the app: <app>/Contents/MacOS/misterdownloadmanager)
# and WORK (an empty folder of its own); OUT is where pictures and logs go.

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACTOOL="${MACTOOL:-$WORK/mactool}"

log() { echo "[$(date +%H:%M:%S)] $*"; }
# <seconds> <command...>: the command, ended after that long (exit 142 when it was).
to() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$s" "$@"; }

mactool_build() {
  [ -x "$MACTOOL" ] && return 0
  swiftc -O -o "$MACTOOL" "$here/mactool.swift" 2>&1 | grep -v "deprecated" || true
  [ -x "$MACTOOL" ]
}
mt() { "$MACTOOL" "$@"; }

# ---- The lab server: neutral addresses (downloads.example, a reserved name, pointed at
# 127.0.0.1 in /etc/hosts, on port 80, which only root may open here: the runner's sudo asks no
# password), else 127.0.0.1:8199. Sets BASE.
lab_start() { # <labserver program>
  local srv=$1 port=8199 host=127.0.0.1 code
  LAB_PROG=$srv; LAB_ROOT=""
  if sudo -n true 2>/dev/null; then
    grep -q 'downloads\.example' /etc/hosts || echo '127.0.0.1 downloads.example' | sudo tee -a /etc/hosts >/dev/null
    sudo dscacheutil -flushcache 2>/dev/null; sudo killall -HUP mDNSResponder 2>/dev/null
    sudo -n "$srv" --bind 127.0.0.1:80 >"$WORK/labserver.log" 2>&1 &
    sleep 1.5
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://downloads.example/gen/1K/check.bin")
    if [ "$code" = 200 ]; then host=downloads.example; port=80; LAB_ROOT=1; fi
  fi
  if [ "$port" != 80 ]; then
    "$srv" --bind "127.0.0.1:$port" >"$WORK/labserver.log" 2>&1 &
    sleep 1
  fi
  BASE="http://$host"; [ "$port" = 80 ] || BASE="http://$host:$port"
  code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/gen/1K/check.bin")
  log "lab server at $BASE answers $code"
  [ "$code" = 200 ]
}
lab_stop() {
  if [ -n "${LAB_ROOT:-}" ]; then sudo -n pkill -f "^$LAB_PROG --bind"; else pkill -f "^$LAB_PROG --bind"; fi 2>/dev/null
  LAB_ROOT=""
}

# ---- MDM's own list and settings: the language chosen, light or dark (`follow`: as the system
# says), every category's folder under the run's own, nothing reaching out (no update check; the
# torrent engine neither listens nor uses the DHT, UPnP or local discovery), no first-start offers.
# [list] is more of the list's file, JSON members beside "settings" ("columns": the list's columns).
mdm_settings() { # <data dir> <downloads dir> <light|dark|follow> [extra settings] [list]
  mkdir -p "$1" "$2"
  python3 - "$1" "$2" "$3" "${4:-}" "${5:-}" <<'PY'
import json, sys
data, dl, mode, extra, top = sys.argv[1:6]
cats = ["General", "Compressed", "Documents", "Music", "Programs", "Video", "Torrents"]
dirs = [[c, dl + ("" if c == "General" else f"/{c}")] for c in cats]
s = dict(language="en", language_asked=True, helpers_asked=True, extension_never_ask=True,
         dark_mode_support=mode != "light", always_dark=mode == "dark", update_check=False,
         torrent_incoming=False, torrent_upnp=False, torrent_dht=False, torrent_lsd=False,
         torrent_stop_idle=True, category_dirs=dirs)
if extra:
    s.update(json.loads("{" + extra + "}"))
d = {"next_id": 1, "items": [], "version": 2, "settings": s}
if top:
    d.update(json.loads("{" + top + "}"))
json.dump(d, open(f"{data}/downloads.json", "w"))
PY
}

# ---- MDM itself. mdm_start sets PID and MAIN (the main window's id); the command line
# (`mdm /show options`) goes to the running copy through its data folder's channel.
mdm() { MDM_DATA_DIR="$DATA" to 30 "$MDM_EXE" "$@" >/dev/null 2>&1; }
mine() { mt windows "$PID" | awk -F'\t' '$3 == 0 && $7 > 60 {print $1}'; }
mdm_start() { # [log file]
  local logf=${1:-$WORK/mdm.log}
  MDM_DATA_DIR="$DATA" "$MDM_EXE" >"$logf" 2>&1 &
  PID=$!
  MAIN=""
  local i
  for i in $(seq 90); do
    kill -0 "$PID" 2>/dev/null || { log "MDM ended at start ($(tail -3 "$logf"))"; return 1; }
    MAIN=$(mine | head -1)
    [ -n "$MAIN" ] && break
    sleep 0.5
  done
  [ -n "$MAIN" ] || { log "no main window in 45 s"; return 1; }
  log "MDM $PID, main window $MAIN after $((i / 2)) s"
  sleep 2
}
# The window that appears after <command...> (a dialog), by id; up to 15 s.
opened() {
  local before new i
  before=$(mine | sort)
  "$@"
  for i in $(seq 30); do
    new=$(comm -13 <(echo "$before") <(mine | sort) | tail -1)
    [ -n "$new" ] && { echo "$new"; return 0; }
    sleep 0.5
  done
  return 1
}
# A window's bounds "x y w h" (points).
bounds() { mt windows "$PID" | awk -F'\t' -v id="$1" '$1 == id {print $4, $5, $6, $7}'; }
# The windows of ours that are menus (MDM's own, drawn above the others): their union "x y w h".
menus_union() { # <x y w h of the window they belong to>
  mt windows "$PID" | awk -F'\t' -v x0="$1" -v y0="$2" -v w0="$3" -v h0="$4" '
    BEGIN {l = x0; t = y0; r = x0 + w0; b = y0 + h0}
    $3 > 0 && $3 < 20 {if ($4 < l) l = $4; if ($5 < t) t = $5; if ($4 + $6 > r) r = $4 + $6; if ($5 + $7 > b) b = $5 + $7}
    END {print l, t, r - l, b - t}'
}
# A window alone, at the display's own pixels: <id> <file> (no shadow) and <file>-shadow.png
# (macOS's shadow, on transparency).
shoot() {
  screencapture -x -o -l "$1" "$2" && echo "  $(basename "$2") $(sips -g pixelWidth -g pixelHeight "$2" | awk '/pixel/ {printf "%s ", $2}')"
  [ -n "${SHADOW:-}" ] && screencapture -x -l "$1" "${2%.png}-shadow.png"
  return 0
}
# How many colours a sample of a picture's pixels has (1: never painted).
colours() {
  sips -s format bmp "$1" --out "$WORK/colours.bmp" >/dev/null 2>&1 || return 1
  python3 -c 'import sys
b = open(sys.argv[1], "rb").read()
at = int.from_bytes(b[10:14], "little")
px = b[at:]
print(len({px[i:i + 3] for i in range(0, len(px) - 3, 4 * 97)}))' "$WORK/colours.bmp"
  rm -f "$WORK/colours.bmp"
}
# The whole screen, or a part of it: <file> [x,y,w,h].
screen() {
  if [ -n "${2:-}" ]; then screencapture -x -R "$2" "$1"; else screencapture -x -m "$1"; fi
  echo "  $(basename "$1") $(sips -g pixelWidth -g pixelHeight "$1" | awk '/pixel/ {printf "%s ", $2}')"
}
park() { mt move "${PARK_X:-1000}" "${PARK_Y:-500}" 2>/dev/null; }
# Our dialog in front closed as Escape closes it (MDM in front first).
escape() { mt activate "$PID" >/dev/null; sleep 0.3; mt key 53; sleep 0.8; }

# ---- Quitting as a person does (/exit, Tasks → Exit: a signal reads as a crash to its monitor),
# timed; one that doesn't quit in 30 s is recorded (#1651) and then stopped. Sets EXIT_SECONDS.
mdm_exit() {
  local t0 i code
  t0=$(perl -MTime::HiRes=time -e 'printf "%.0f", time*1000')
  mdm /exit
  for i in $(seq 300); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
  EXIT_SECONDS=$(perl -MTime::HiRes=time -e "printf '%.1f', (time*1000 - $t0)/1000")
  if kill -0 "$PID" 2>/dev/null; then
    log "MDM didn't quit 30 s after /exit (#1651): stopped"
    EXIT_SECONDS="no ($EXIT_SECONDS s)"
    kill "$PID" 2>/dev/null; sleep 2; kill -9 "$PID" 2>/dev/null
    return 1
  fi
  code=0; wait "$PID" 2>/dev/null || code=$?
  # 127: not this shell's child (started through `open`), its code unknown.
  [ "$code" = 127 ] && code="unknown"
  log "MDM quit $EXIT_SECONDS s after /exit, exit code $code"
  return 0
}

# ---- The system's look: dark or light (System Events, else the global preference).
appearance() {
  local v=false; [ "$1" = dark ] && v=true
  osascript -e "tell application \"System Events\" to tell appearance preferences to set dark mode to $v" 2>/dev/null \
    || mt appearance "$1"
  sleep 2
}

# ---- The runner's screen: 1024x768 at one pixel a point, black. The sharpest it can be made:
# a virtual display of <w>x<h> points at two pixels a point (vdisplay.m, HiDPI as on a Retina
# Mac), else the runner's own display at <w>x<h> (one pixel a point). Sets SW, SH (points), SCALE.
display_setup() { # <w> <h>
  local w=$1 h=$2
  if [ -z "${MDM_NO_VDISPLAY:-}" ] && clang -fobjc-arc -framework Foundation -framework CoreGraphics \
       -o "$WORK/vdisplay" "$here/vdisplay.m" 2>"$WORK/vdisplay-build.txt"; then
    "$WORK/vdisplay" "$w" "$h" >"$WORK/vdisplay.txt" 2>&1 &
    VDISPLAY_PID=$!
    for _ in $(seq 40); do
      grep -q '^ready' "$WORK/vdisplay.txt" && break
      kill -0 "$VDISPLAY_PID" 2>/dev/null || break
      sleep 0.5
    done
    log "virtual display: $(tr '\n' ' ' <"$WORK/vdisplay.txt")"
  else
    log "no virtual display: $(tail -3 "$WORK/vdisplay-build.txt" 2>/dev/null)"
  fi
  sleep 3
  read -r SW SH SCALE < <(main_display)
  if [ "${SCALE:-1}" != 2 ] || [ "$SW" != "$w" ]; then
    log "no HiDPI display (${SW}x$SH at $SCALE): the runner's own at ${w}x$h"
    display_restore; sleep 2
    mt mode "$w" "$h" "$w"; sleep 3
    read -r SW SH SCALE < <(main_display)
  fi
  PARK_X=$((SW - 2)); PARK_Y=$((SH / 2))
  log "screen ${SW}x${SH} points at $SCALE pixels a point"
}
# The main display (the one with the menu bar): "w h scale".
main_display() {
  mt displays | awk '/^display/ && /main=true/ {
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^now=/) split(substr($i, 5), p, "x")
        if ($i ~ /^pixels=/) split(substr($i, 8), q, "x")
      }
      printf "%d %d %d\n", p[1], p[2], q[1] / p[1]; exit }'
}
display_restore() { [ -n "${VDISPLAY_PID:-}" ] && kill "$VDISPLAY_PID" 2>/dev/null; VDISPLAY_PID=""; }

# A wallpaper of macOS's own instead of the runner's black: $MDM_WALLPAPER, else the first of the
# system's pictures named after a macOS version, newest first.
wallpaper() {
  local pic="${MDM_WALLPAPER:-}" name
  if [ -z "$pic" ]; then
    for name in Tahoe Sequoia Sonoma Ventura Monterey; do
      pic=$(ls -d "/System/Library/Desktop Pictures/$name"*.heic 2>/dev/null | head -1)
      [ -n "$pic" ] && break
    done
  fi
  [ -n "$pic" ] || { log "wallpaper: none of macOS's found"; return 0; }
  osascript -e "tell application \"System Events\" to tell every desktop to set picture to \"$pic\"" >/dev/null 2>&1
  log "wallpaper: $pic"
  sleep 2
}

# ---- The menu bar item (the status item: MDM's icon where Windows has its notification area):
# its bounds "x y w h", from System Events (accessibility), else from the window list (its window,
# MDM's, in the status items' layer; macOS 26 keeps it out of the app's own list).
menu_item_bounds() {
  local b
  b=$(osascript -e "tell application \"System Events\" to tell (first process whose unix id is $PID) to get {position, size} of menu bar item 1 of menu bar 2" 2>/dev/null \
    | tr -d ' ' | awk -F, 'NF == 4 {print $1, $2, $3, $4}')
  [ -n "$b" ] || b=$(mt windows "$PID" | awk -F'\t' '$3 == 25 {print $4, $5, $6, $7; exit}')
  echo "$b"
}

# ---- The runner's desktop for pictures: no Dock (hidden until pointed at), no desktop icons, no
# notification banners (Do Not Disturb), the pointer parked at the right edge.
desktop_quiet() {
  defaults write com.apple.dock autohide -bool true
  defaults write com.apple.dock autohide-delay -float 1000
  killall Dock 2>/dev/null
  defaults write com.apple.finder CreateDesktop -bool false
  killall Finder 2>/dev/null
  # Do Not Disturb (Focus): no banner over a picture; none is expected on a runner.
  defaults -currentHost write com.apple.notificationcenterui doNotDisturb -boolean true 2>/dev/null
  killall NotificationCenter 2>/dev/null
  sleep 2
  park
}
