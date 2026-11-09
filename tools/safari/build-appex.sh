#!/usr/bin/env bash
# Builds the Safari extension (an app extension, "appex") into a Mister Download Manager.app:
#   tools/safari/build-appex.sh <app> <swift target> <extension folder> <helper> <version> [identity]
#     <app>               the .app to put it in (Contents/PlugIns/)
#     <swift target>      arm64-apple-macos14.0 or x86_64-apple-macos14.0
#     <extension folder>  target/extension/safari (tools/extension/pack.ps1): the web extension,
#                         the same files as for the other browsers
#     <helper>            the release misterdownloadmanager-nmhost for that processor: copied in
#                         and signed to run inside the extension's sandbox (the app's own copy,
#                         which the other browsers start, must not carry that signature)
#     [identity]          codesign identity; "-" (ad hoc) when left out
# No Xcode project: the extension is one Swift file (SafariWebExtensionHandler.swift) and a plist,
# compiled with swiftc, so there is nothing to keep in step with Xcode's versions. Sign the app
# after this (the extension is sealed into it).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
app="$1" target="$2" ext="$3" helper="$4" version="$5" identity="${6:--}"
min_macos="${target##*macos}"
bundle_id="io.github.anefzaoui.misterdownloadmanager.safari"
appex="$app/Contents/PlugIns/Mister Download Manager.appex"

# A real identity: the hardened runtime and Apple's timestamp, which notarization asks for.
opts=()
[ "$identity" = "-" ] || opts=(--options runtime --timestamp)

rm -rf "$appex"
mkdir -p "$appex/Contents/MacOS" "$appex/Contents/Resources"

# -e _NSExtensionMain: an app extension starts in the system's extension host, not in main().
xcrun swiftc -O -parse-as-library -application-extension \
  -module-name MisterDownloadManagerExtension -target "$target" \
  -framework SafariServices -Xlinker -e -Xlinker _NSExtensionMain \
  -o "$appex/Contents/MacOS/MisterDownloadManagerExtension" \
  "$here/SafariWebExtensionHandler.swift"

sed -e "s|@BUNDLE_ID@|$bundle_id|" -e "s|@VERSION@|$version|" -e "s|@MIN_MACOS@|$min_macos|" \
  "$here/Info.plist.in" > "$appex/Contents/Info.plist"
cp -R "$ext/." "$appex/Contents/Resources/"
cp "$helper" "$appex/Contents/MacOS/misterdownloadmanager-nmhost"

# Inside the sandbox the extension has: the helper takes it on (com.apple.security.inherit).
codesign --force --sign "$identity" ${opts[@]+"${opts[@]}"} \
  --entitlements "$here/helper.entitlements" "$appex/Contents/MacOS/misterdownloadmanager-nmhost"
codesign --force --sign "$identity" ${opts[@]+"${opts[@]}"} \
  --entitlements "$here/extension.entitlements" "$appex"
