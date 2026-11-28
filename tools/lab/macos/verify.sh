#!/bin/bash
# A pull request (or any commit) checked on macOS, as a person runs it, on GitHub's macOS runner
# (.github/workflows/macos-showcase.yml, the verify job): MDM started with its own data folder and
# download folder, a download from the lab server run to its end in the window (8 connections),
# its file checked against the server's (SHA-256), the automation API's answer about it, the
# command line's own download (`--cli get`) checked the same way, pictures of the main window and
# the progress window, MDM's log, and MDM quit with /exit, timed: once with only the main window,
# once with a dialog open and the main window minimized (#1651).
#
#   verify.sh <Mister Download Manager.app> <labserver program> <out folder>
#
# <out>/verify.txt is the summary, one PASS or FAIL line a check; the exit code is 1 when any
# check failed.
set -u
APP=$1; LABSRV=$2; OUT=$3
WORK="$HOME/Verify"
rm -rf "$WORK"; mkdir -p "$WORK" "$OUT"
MDM_EXE="$APP/Contents/MacOS/misterdownloadmanager"
. "$(dirname "$0")/lib.sh"
sum="$OUT/verify.txt"; : >"$sum"
failed=0
check() { # <PASS|FAIL> <text>
  echo "$1 $2" | tee -a "$sum"
  [ "$1" = PASS ] || failed=1
}
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

mactool_build || { check FAIL "mactool didn't build"; exit 1; }
{ sw_vers; uname -m; mt access; mt displays | head -1; } >"$OUT/runner.txt" 2>&1
park
lab_start "$LABSRV" || { check FAIL "lab server"; exit 1; }

DATA="$WORK/data"; DL="$WORK/Downloads"
mdm_settings "$DATA" "$DL" follow '"show_complete":false'
if ! mdm_start "$OUT/mdm-stdout.log"; then
  check FAIL "MDM started and drew its main window"
  screencapture -x "$OUT/screen-no-window.png"
  cp -R "$DATA/logs" "$OUT/logs" 2>/dev/null
  exit 1
fi
check PASS "MDM started and drew its main window ($(bounds "$MAIN" | awk '{print $3 "x" $4}') points)"

# ---- A download in the window: 200 MB at 1 MB/s a connection, so its 8 connections show.
name=verify-200M.bin
url="$BASE/gen/200M/$name?rate=1M"
t0=$(date +%s)
if prog=$(opened mdm /d "$url" /p "$DL" /n); then
  sleep 5; park
  shoot "$MAIN" "$OUT/main-downloading.png"
  shoot "$prog" "$OUT/progress.png"
  check PASS "the progress window opened"
else
  check FAIL "the progress window opened"
fi
for _ in $(seq 240); do
  [ -f "$DL/$name" ] && [ ! -e "$DL/$name.mdm.part" ] && break
  sleep 1
done
secs=$(( $(date +%s) - t0 ))
if [ -f "$DL/$name" ]; then
  curl -s "$BASE/gen/200M/$name" -o "$WORK/expected.bin"
  if [ "$(sha "$DL/$name")" = "$(sha "$WORK/expected.bin")" ]; then
    check PASS "the window's download finished in ${secs} s and its SHA-256 matches the server's ($(sha "$DL/$name"))"
  else
    check FAIL "the window's download's SHA-256 ($(sha "$DL/$name")) differs from the server's ($(sha "$WORK/expected.bin"))"
  fi
  rm -f "$WORK/expected.bin"
else
  check FAIL "the window's download finished in 240 s ($(ls -l "$DL" | tail -n +2 | tr '\n' ';'))"
fi
sleep 2; park
shoot "$MAIN" "$OUT/main-done.png"

# ---- The automation API: on, then the download's status as it reports it.
cli() { MDM_DATA_DIR="$DATA" to 60 "$MDM_EXE" --cli "$@"; }
cli api on >"$OUT/api-on.txt" 2>&1
cli --json status >"$OUT/api-status.json" 2>&1
if python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); x=[i for i in d["downloads"] if i["file"]==sys.argv[2]]; sys.exit(0 if x and x[0]["status"]=="Complete" else 1)' "$OUT/api-status.json" "$name"; then
  check PASS "the automation API lists it as Complete"
else
  check FAIL "the automation API lists it as Complete ($(head -c 300 "$OUT/api-status.json"))"
fi

# ---- The command line's own download (no window).
mkdir -p "$WORK/cli"
if cli --json get "$BASE/gen/64M/cli-64M.bin" -o "$WORK/cli" >"$OUT/cli-get.jsonl" 2>&1 \
   && [ "$(sha "$WORK/cli/cli-64M.bin")" = "$(curl -s "$BASE/gen/64M/cli-64M.bin" | shasum -a 256 | awk '{print $1}')" ]; then
  check PASS "--cli get downloaded 64 MB and its SHA-256 matches ($(tail -1 "$OUT/cli-get.jsonl"))"
else
  check FAIL "--cli get ($(tail -2 "$OUT/cli-get.jsonl" | tr '\n' ' '))"
fi

# ---- Quitting: with only the main window, then with a dialog open and the main window
# minimized (#1651: on Windows /exit sometimes didn't quit then).
if mdm_exit; then check PASS "/exit quit MDM in $EXIT_SECONDS s (main window only)"; else check FAIL "/exit quit MDM (main window only): $EXIT_SECONDS"; fi
if mdm_start "$OUT/mdm-stdout-2.log"; then
  opened mdm /show about >/dev/null || log "About: no window"
  mdm /show minimized; sleep 2
  mt windows "$PID" >"$OUT/windows-before-exit.txt"
  if mdm_exit; then check PASS "/exit quit MDM in $EXIT_SECONDS s (About open, main window minimized)"; else check FAIL "/exit quit MDM (About open, main window minimized, #1651): $EXIT_SECONDS"; fi
else
  check FAIL "MDM started a second time"
fi

lab_stop
cp -R "$DATA/logs" "$OUT/logs" 2>/dev/null
ls "$DATA"/*.log >/dev/null 2>&1 && cp "$DATA"/*.log "$OUT/"
rm -rf "$DL" "$WORK/cli"
echo; cat "$sum"
exit $failed
