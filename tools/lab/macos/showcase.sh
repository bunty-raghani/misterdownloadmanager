#!/bin/bash
# MDM on macOS, shown on GitHub's macOS runner (.github/workflows/macos-showcase.yml): an MDM of
# its own (its own data folder and download folder) with a list that looks used, from its own lab
# server at a neutral address; every window opened through MDM's own command line (`/show`),
# light and dark (the system's appearance, which MDM follows), the style themes, the menu bar
# item, and a short screen recording. Nothing personal is on a runner; nothing of another
# program shows but macOS's own menu bar and wallpaper.
#
#   showcase.sh <Mister Download Manager.app> <labserver program> <out folder>
#
# <out>/runner.txt says what the runner had (macOS, processor, displays, what this process may
# do); <out>/light, <out>/dark: the windows (screencapture -l: one window, at the display's own
# pixels, without its shadow; *-shadow.png with it, on transparency) and whole screens;
# <out>/themes-light, <out>/themes-dark: the main window (and the progress window) in each style
# theme; <out>/recording: the recording and what it measured.
#
# MDM_SHOWCASE_SIZE="1920 1080": the screen, in points (HiDPI when the virtual display works).
set -u
APP=$1; LABSRV=$2; OUT=$3
WORK="$HOME/Demo"
rm -rf "$WORK"; mkdir -p "$WORK" "$OUT"
MDM_EXE="$APP/Contents/MacOS/misterdownloadmanager"
. "$(dirname "$0")/lib.sh"
SHADOW=1

mactool_build || { log "mactool didn't build"; exit 1; }

# ---- What the runner has
{
  sw_vers; uname -m; sysctl -n machdep.cpu.brand_string hw.memsize hw.ncpu
  echo; mt access
  echo; mt displays
  echo; echo "macOS's wallpapers:"; ls "/System/Library/Desktop Pictures" 2>&1 | head -40
} >"$OUT/runner.txt" 2>&1

# shellcheck disable=SC2086
display_setup ${MDM_SHOWCASE_SIZE:-1920 1080} | tee -a "$OUT/runner.txt"
read -r SW SH SCALE < <(main_display)
PARK_X=$((SW - 2)); PARK_Y=$((SH / 2))
desktop_quiet
wallpaper | tee -a "$OUT/runner.txt"
lab_start "$LABSRV" || { log "no lab server"; exit 1; }

# The main window: a size that shows every column of a lived-in list, centred a little high.
WIN_W=1180; WIN_H=640
WIN_X=$(( (SW - WIN_W) / 2 )); WIN_Y=$(( (SH - WIN_H) / 2 - 40 ))
extra="\"show_complete\":false,\"show_progress\":false,\"window\":[$WIN_X,$WIN_Y,$WIN_W,$WIN_H]"
# File Name wide enough for the whole names (crates/app/src/list.rs: DEFAULT_WIDTHS, DEFAULT_ORDER).
columns='"columns":{"order":[9,0,1,2,3,4,5,12,10,11,6,8,7],"widths":[260,20,75,132,75,95,65,200,145,62,64,92,100],"visible":[true,true,true,true,true,true,true,true,true,true,true,true,true],"sort":null,"status_fits":true}'

# A list that looks used: finished files, one still coming in (rate-limited per connection: 1M
# over 8 connections is about 8 MB/s, so the progress window shows its 8 connections and the
# segment map filling for minutes).
fill_list() {
  local f name size q
  for f in big-buck-bunny-1080p.mp4:180M project-report-2026.pdf:4M holiday-photos.zip:64M \
           podcast-episode-42.mp3:38M rust-1.99.1-aarch64-apple-darwin.tar.xz:92M \
           open-data-2026.csv:12M "ubuntu-26.04-desktop-arm64.iso:6G:?rate=1M"; do
    IFS=: read -r name size q <<< "$f"
    mdm /d "$BASE/gen/$size/$name${q:-}" /p "$WORK/Downloads" /n
    sleep 0.4
  done
}

# The menu bar item (MDM's icon where Windows has its notification area), from the right of the
# menu bar. Its panel and menu aren't taken: on the runners a click posted there (left, right, or
# System Events' "show menu") opens neither, three rounds of tries in a row.
menu_bar_item() { # <dir>
  local x y w h
  read -r x y w h < <(menu_item_bounds)
  if [ -z "${h:-}" ]; then log "no menu bar item"; mt windows >"$1/windows-all.txt"; return; fi
  log "menu bar item at $x,$y ${w}x$h"
  screen "$1/menu-bar.png" "$((SW - 560)),0,560,$((y + h + 2))"
}

for mode in light dark; do
  dir="$OUT/$mode"; mkdir -p "$dir"
  log "---- $mode"
  appearance "$mode"
  DATA="$WORK/data-$mode"; rm -rf "$DATA" "$WORK/Downloads"
  mdm_settings "$DATA" "$WORK/Downloads" follow "$extra" "$columns"
  mdm_start "$dir/mdm.log" || { mt windows >"$dir/windows.txt"; screen "$dir/screen-no-window.png"; continue; }
  fill_list
  sleep 25
  park; sleep 1
  screen "$dir/screen-main.png"
  shoot "$MAIN" "$dir/main.png"
  # (Early, while MDM has little to do: System Events asks MDM itself where its item is.)
  [ "$mode" = light ] && menu_bar_item "$dir"
  # View → Theme open (MDM's own menus: the main window and the menus, from the screen).
  mdm /show menu:3:theme; sleep 2
  read -r x y w h < <(bounds "$MAIN")
  read -r ux uy uw uh < <(menus_union "$x" "$y" "$w" "$h")
  if [ -n "${uh:-}" ]; then screen "$dir/menu-theme.png" "$ux,$uy,$uw,$uh"; else mt windows >"$dir/windows-menu.txt"; log "menu-theme: main window at '$x $y $w $h'"; fi
  mt key 53; sleep 0.5; mt key 53; sleep 1
  # The windows, by name, each closed (Escape) before the next; the progress window last, as
  # it stays for the screen with it and for the themes.
  for pair in options=options:connection add-url=addurl scheduler=scheduler safari=safari-steps \
              about=about speed-limiter=speedlimit progress=progress; do
    name=${pair%%=*}; show=${pair#*=}
    if w=$(opened mdm /show "$show"); then
      sleep 2; park; shoot "$w" "$dir/$name.png"
      [ "$name" = progress ] && PROG=$w || escape
    else
      log "$name: no window"
    fi
  done
  sleep 1; park
  screen "$dir/screen-progress.png"
  # The style themes: the main window in each, and the progress window in a few.
  tdir="$OUT/themes-$mode"; mkdir -p "$tdir"
  for t in aqua metro emerald luna aero adwaita breeze native high-contrast classic; do
    mdm /show "theme:$t"; sleep 2.5; park
    shoot "$MAIN" "$tdir/main-$t.png"
    case $t in aqua|metro|luna|aero|emerald)
      mdm /show progress; sleep 2; park
      [ -n "${PROG:-}" ] && shoot "$PROG" "$tdir/progress-$t.png" ;;
    esac
  done
  mdm /show progress; sleep 1; park
  screen "$dir/screen-end.png"
  mt windows "$PID" >"$dir/windows-end.txt"
  mdm_exit; echo "exit after /exit: $EXIT_SECONDS s ($mode, progress window open)" >>"$OUT/runner.txt"
done

# ---- The recording (screencapture -v, macOS's own, frames as the screen changes, up to 60 a
# second): the list as it is, the style themes switched, the progress window opening on the
# running download, then Options.
rdir="$OUT/recording"; mkdir -p "$rdir"
appearance light
DATA="$WORK/data-rec"; rm -rf "$DATA" "$WORK/Downloads"
mdm_settings "$DATA" "$WORK/Downloads" follow "$extra" "$columns"
if mdm_start "$rdir/mdm.log"; then
  fill_list; sleep 20; park
  log "recording"
  screencapture -x -v -V 26 "$rdir/screencapture.mov" >"$rdir/screencapture.log" 2>&1 &
  rec=$!
  sleep 2.5
  for t in aqua metro emerald classic; do mdm /show "theme:$t"; sleep 2.8; done
  mdm /show progress; sleep 7
  mdm /show options:connection; sleep 4
  for _ in $(seq 40); do kill -0 "$rec" 2>/dev/null || break; sleep 1; done
  sleep 2
  for f in "$rdir"/*.mov; do
    [ -f "$f" ] && log "$(basename "$f"): $(stat -f %z "$f") bytes"
  done
  mdm_exit
fi

lab_stop
appearance light
display_restore
rm -rf "$WORK/Downloads"
log "pictures in $OUT"
