#!/bin/bash
# A macOS .dmg of MDM checked as a person installs it, on GitHub's macOS runner
# (.github/workflows/macos-showcase.yml, the dmg job): mounted, the app copied to /Applications,
# its signature and Gatekeeper's verdict recorded, started through Launch Services (as a double
# click does) with its own data folder, its window drawn (a picture), a download over 8
# connections from a local server and one over HTTPS from GitHub, each checked by its SHA-256, the
# Safari extension's registration, and MDM quit with /exit. The runner's quarantine flag is
# removed first (a browser's download would carry it; Gatekeeper's question is a person's).
#
#   dmg-check.sh <.dmg> <out folder> [<https address> <its expected SHA-256>]
#
# <out>/dmg.txt is the summary, one PASS or FAIL line a check; the exit code is 1 when any failed.
set -u
DMG=$1; OUT=$2; HTTPS_URL=${3:-}; HTTPS_SHA=${4:-}
WORK="$HOME/DmgCheck"
rm -rf "$WORK"; mkdir -p "$WORK" "$OUT"
. "$(dirname "$0")/lib.sh"
sum="$OUT/dmg.txt"; : >"$sum"
failed=0
check() { echo "$1 $2" | tee -a "$sum"; [ "$1" = PASS ] || failed=1; }
note() { echo "NOTE $1" | tee -a "$sum"; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

mactool_build || { check FAIL "mactool didn't build"; exit 1; }
note "runner: $(sw_vers -productVersion) $(uname -m), $(basename "$DMG")"
park

# ---- Mount and install
mnt="$WORK/mnt"; mkdir -p "$mnt"
if hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$DMG" >"$OUT/hdiutil.txt" 2>&1; then
  check PASS "the .dmg mounts ($(ls "$mnt" | tr '\n' ',' ))"
else
  check FAIL "the .dmg mounts ($(tail -2 "$OUT/hdiutil.txt"))"; exit 1
fi
src="$mnt/Mister Download Manager.app"
[ -L "$mnt/Applications" ] && note "beside the app: a link to $(readlink "$mnt/Applications")"
app="/Applications/Mister Download Manager.app"
sudo rm -rf "$app"
if ditto "$src" "$app"; then check PASS "the app copies to /Applications"; else check FAIL "the app copies to /Applications"; exit 1; fi
hdiutil detach "$mnt" -quiet || hdiutil detach "$mnt" -force -quiet
note "quarantine on the copy before: $(xattr -p com.apple.quarantine "$app" 2>/dev/null || echo none)"
xattr -dr com.apple.quarantine "$app" 2>/dev/null

exe="$app/Contents/MacOS/misterdownloadmanager"
note "program: $(file -b "$exe" | head -1)"
note "Info.plist: $(defaults read "$app/Contents/Info" CFBundleShortVersionString 2>/dev/null), minimum macOS $(defaults read "$app/Contents/Info" LSMinimumSystemVersion 2>/dev/null)"
if codesign --verify --deep --strict "$app" >"$OUT/codesign-verify.txt" 2>&1; then
  check PASS "codesign --verify --deep --strict"
else
  check FAIL "codesign --verify --deep --strict ($(tail -2 "$OUT/codesign-verify.txt" | tr '\n' ' '))"
fi
codesign -dvvv "$app" >"$OUT/codesign-display.txt" 2>&1
note "signature: $(grep -E '^(Signature|Authority|TeamIdentifier)' "$OUT/codesign-display.txt" | tr '\n' ' ')"
spctl --assess --type execute -vv "$app" >"$OUT/spctl.txt" 2>&1
note "Gatekeeper (spctl): $(tr '\n' ' ' <"$OUT/spctl.txt")"
for p in misterdownloadmanager-cli misterdownloadmanager-nmhost misterdownloadmanager-mcp; do
  [ -x "$app/Contents/MacOS/$p" ] && note "$p: $(file -b "$app/Contents/MacOS/$p" | head -1)"
done

# ---- Started as a double click starts it (Launch Services), with its own data folder.
DATA="$WORK/data"; DL="$WORK/Downloads"
mdm_settings "$DATA" "$DL" follow '"show_complete":false'
MDM_EXE="$exe"
if open -n -a "$app" --env "MDM_DATA_DIR=$DATA" >"$OUT/open.txt" 2>&1; then
  how="open (Launch Services)"
else
  how="the program directly (open: $(tr '\n' ' ' <"$OUT/open.txt"))"
  MDM_DATA_DIR="$DATA" "$exe" >"$OUT/mdm-stdout.log" 2>&1 &
fi
PID=""; MAIN=""
for _ in $(seq 90); do
  PID=$(pgrep -f "^$exe" | head -1)
  [ -n "$PID" ] && MAIN=$(mine | head -1) && [ -n "$MAIN" ] && break
  sleep 0.5
done
if [ -n "$MAIN" ]; then
  sleep 3; park
  check PASS "started through $how; its main window drew ($(bounds "$MAIN" | awk '{print $3 "x" $4}') points)"
  shoot "$MAIN" "$OUT/main.png"
  screen "$OUT/screen.png"
  # It drew: its picture has many colours, not one (a window opened but never painted).
  n=$(colours "$OUT/main.png")
  if [ "${n:-0}" -gt 8 ]; then check PASS "its window is painted ($n colours sampled)"; else check FAIL "its window is painted (${n:-?} colours sampled)"; fi
else
  check FAIL "started through $how; its main window drew (process: ${PID:-none})"
  screen "$OUT/screen-no-window.png"
  mt windows >"$OUT/windows.txt"
fi

if [ -n "$MAIN" ]; then
  # ---- A download over 8 connections from a local server (ranges), checked.
  srv="$WORK/srv"; mkdir -p "$srv"
  head -c 134217728 /dev/urandom >"$srv/sample-128M.bin"
  python3 "$(dirname "$0")/rangeserver.py" "$srv" 8197 >"$WORK/rangeserver.log" 2>&1 &
  rs=$!; sleep 1
  t0=$(date +%s)
  prog=$(opened mdm /d "http://127.0.0.1:8197/sample-128M.bin" /p "$DL" /n) && { sleep 2; shoot "$prog" "$OUT/progress.png"; }
  for _ in $(seq 180); do [ -f "$DL/sample-128M.bin" ] && [ ! -e "$DL/sample-128M.bin.mdm.part" ] && break; sleep 1; done
  if [ -f "$DL/sample-128M.bin" ] && [ "$(sha "$DL/sample-128M.bin")" = "$(sha "$srv/sample-128M.bin")" ]; then
    check PASS "a 128 MB download from a local server, in $(( $(date +%s) - t0 )) s, matches by SHA-256"
  else
    check FAIL "a 128 MB download from a local server ($(ls -l "$DL" 2>&1 | tail -n +2 | tr '\n' ';'))"
  fi
  kill "$rs" 2>/dev/null
  # ---- One over HTTPS from GitHub (its redirect to the file's storage, TLS), checked.
  if [ -n "$HTTPS_URL" ]; then
    f=$(basename "$HTTPS_URL")
    mdm /d "$HTTPS_URL" /p "$DL" /n
    for _ in $(seq 120); do [ -f "$DL/$f" ] && [ ! -e "$DL/$f.mdm.part" ] && break; sleep 1; done
    if [ -f "$DL/$f" ] && [ "$(sha "$DL/$f")" = "$HTTPS_SHA" ]; then
      check PASS "an HTTPS download from GitHub ($f) matches by SHA-256"
    else
      check FAIL "an HTTPS download from GitHub ($f; $(ls -l "$DL" 2>&1 | tail -n +2 | tr '\n' ';'))"
    fi
  fi
  sleep 2; park
  shoot "$MAIN" "$OUT/main-done.png"
  if mdm_exit; then check PASS "/exit quit MDM in $EXIT_SECONDS s"; else check FAIL "/exit quit MDM: $EXIT_SECONDS"; fi
fi

# ---- The Safari extension: registered with the system once the app is.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" 2>/dev/null
ls "$app/Contents/PlugIns" >"$OUT/plugins.txt" 2>&1
sleep 2
{
  echo "== Safari's web extensions"; pluginkit -mAvvv -p com.apple.Safari.web-extension
  echo "== the extension's signature"; codesign -dvvv --entitlements - "$app/Contents/PlugIns/"*.appex
} >"$OUT/pluginkit.txt" 2>&1
# Decided from that one listing (a second query taken right after re-adding it by hand once found
# none for a moment: a check's race, not the app's).
if sed -n '1,/^ *(/p' "$OUT/pluginkit.txt" | grep -q "anefzaoui"; then
  check PASS "Safari's extension is registered ($(grep -m1 anefzaoui "$OUT/pluginkit.txt" | xargs))"
else
  check FAIL "Safari's extension is registered (PlugIns: $(tr '\n' ' ' <"$OUT/plugins.txt"))"
fi

# ---- What the system logged about it, and any crash report.
/usr/bin/log show --last 10m --style compact --predicate 'process == "misterdownloadmanager" AND messageType >= 16' >"$OUT/system-log.txt" 2>&1
ls ~/Library/Logs/DiagnosticReports 2>/dev/null | grep -i misterdownload >"$OUT/crash-reports.txt" && {
  note "crash reports: $(tr '\n' ' ' <"$OUT/crash-reports.txt")"
  cp ~/Library/Logs/DiagnosticReports/*misterdownload* "$OUT/" 2>/dev/null
}
cp -R "$DATA/logs" "$OUT/logs" 2>/dev/null
pkill -f "^$exe" 2>/dev/null
sudo rm -rf "$app" "$DL" "$WORK/srv"
echo; cat "$sum"
exit $failed
