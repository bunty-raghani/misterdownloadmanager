#!/usr/bin/env bash
# Builds the Linux release for this machine's processor into target/dist/ (up to 0.2.4, each
# name ended in "-experimental"), <arch> being x86_64 on a 64-bit Intel or AMD machine and arm64
# on a 64-bit ARM one:
#   MisterDownloadManager-<version>-linux-<arch>.AppImage        runs on most distros
#   MisterDownloadManager-<version>-linux-<arch>.AppImage.zsync  its updates (AppImage tools)
#   MisterDownloadManager-<version>-linux-<arch>.tar.gz          the plain programs
#   MisterDownloadManager-<version>-linux-<arch>.deb             Debian, Ubuntu, Mint
#   MisterDownloadManager-<version>-linux-<arch>.rpm             Fedora, RHEL, openSUSE
# Update now (crates/tools/src/update.rs), the website (tools/website/build.py) and the docs
# look for these names, and still for the "-experimental" ones of older releases.
# Needs: cargo, pwsh (the extension), python3 with Pillow (the icon), dpkg-deb, rpmbuild, network access (appimagetool is fetched),
# and zsyncmake if the appimagetool fetched doesn't write the .zsync itself.
# Build on the oldest Ubuntu you want to support: the programs need its glibc or newer.
#   tools/installer/build-linux.sh [--no-build]
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
dist="$root/target/dist"
bin="$root/target/release"
version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
# The processor as the files' names say it (arch), and as Debian (deb_arch), RPM and AppImage
# (pkg_arch) do.
case "$(uname -m)" in
  x86_64) arch=x86_64 deb_arch=amd64 pkg_arch=x86_64 ;;
  aarch64) arch=arm64 deb_arch=arm64 pkg_arch=aarch64 ;;
  *) echo "no Linux release is built for $(uname -m)" >&2; exit 1 ;;
esac
name="MisterDownloadManager-$version-linux-$arch"
programs=(misterdownloadmanager misterdownloadmanager-nmhost misterdownloadmanager-cli misterdownloadmanager-mcp)

if [ "${1:-}" != "--no-build" ]; then
  # The build machine's paths out of the programs (#25): panic locations and debug info say
  # "cargo/..." for cargo's folder and "mdm/..." for this checkout. Encoded flags (unit separators),
  # as those paths may hold spaces.
  CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo"$'\x1f'"--remap-path-prefix=$root=mdm" \
    cargo build --release -p mdm-app -p mdm-nmhost -p mdm-cli -p mdm-mcp
fi
pwsh -NoProfile -File "$root/tools/extension/pack.ps1"

mkdir -p "$dist"
stage="$dist/stage-linux"
rm -rf "$stage"
mkdir -p "$stage"

# One tree in the usual Linux layout; every package is made from it.
tree="$stage/tree"
mkdir -p "$tree/usr/bin" "$tree/usr/share/applications" \
  "$tree/usr/share/icons/hicolor/256x256/apps" "$tree/usr/share/misterdownloadmanager" \
  "$tree/usr/share/doc/misterdownloadmanager"
for p in "${programs[@]}"; do install -m 755 "$bin/$p" "$tree/usr/bin/$p"; done
# Crash reports: the release's symbols from the programs' debug info (the -symbols zip), then the
# programs without it or their symbol table (#25; the build id that matches them to the symbols,
# and the unwind tables, stay).
python3 "$root/tools/installer/symbols.py" "linux-$arch" "$version" "$dist" "${programs[@]/#/$bin/}"
for p in "${programs[@]}"; do strip --strip-all "$tree/usr/bin/$p"; done
# The oldest glibc the programs run on: the newest GLIBC_x.y symbol version they ask for. The
# .deb and .rpm require it, so a system too old refuses the package instead of installing
# programs that can't start ("GLIBC_2.38 not found" on Debian 12 from a build on Ubuntu 24.04).
glibc=$(for p in "${programs[@]}"; do objdump -T "$tree/usr/bin/$p"; done \
  | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)
[ -n "$glibc" ] || { echo "no glibc version found in the programs" >&2; exit 1; }
echo "the programs need glibc $glibc or newer"
cp -r "$root/target/extension/chromium" "$tree/usr/share/misterdownloadmanager/extension-chromium"
cp "$root/target/extension/mdm-firefox.xpi" "$tree/usr/share/misterdownloadmanager/"
cp "$root/LICENSE" "$root/ATTRIBUTION.md" "$tree/usr/share/doc/misterdownloadmanager/"
python3 - "$root/art/Mister Download Manager.png" \
  "$tree/usr/share/icons/hicolor/256x256/apps/misterdownloadmanager.png" <<'EOF'
import sys
from PIL import Image
Image.open(sys.argv[1]).convert("RGBA").resize((256, 256), Image.LANCZOS).save(sys.argv[2])
EOF
# StartupWMClass: the class every window of MDM carries (main.rs APP_ID), so a window pinned to
# the taskbar or the dock is this entry's, whatever name an AppImage manager gives the entry (#940).
cat > "$tree/usr/share/applications/misterdownloadmanager.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Mister Download Manager
Comment=Download manager
Exec=misterdownloadmanager %U
Icon=misterdownloadmanager
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=misterdownloadmanager
MimeType=x-scheme-handler/magnet;application/x-bittorrent;
Keywords=mdm;idm;download manager;
EOF

# tar.gz: the programs, the extension and the licence in one folder.
plain="$stage/$name"
mkdir -p "$plain"
for p in "${programs[@]}"; do cp "$tree/usr/bin/$p" "$plain/"; done
cp -r "$tree/usr/share/misterdownloadmanager/extension-chromium" "$plain/extension-chromium"
cp "$tree/usr/share/misterdownloadmanager/mdm-firefox.xpi" "$plain/"
cp "$root/LICENSE" "$root/ATTRIBUTION.md" "$plain/"
tar -C "$stage" -czf "$dist/$name.tar.gz" "$name"

# .deb. Its dependencies, and the .rpm's: the libraries the window loads at run time. Without
# libxkbcommon-x11 (winit's keyboard under X11, and XWayland, MDM's default in a Wayland session)
# the window panics at start; EGL draws Wayland windows; without the CA certificates no download
# starts, HTTP ones too ("No CA certificates were loaded from the system"). GTK pulls in the rest.
# The browsers' host manifests for everyone (#266), in the .deb and .rpm only: each folder the
# browsers read them from (/etc/opt/chrome, /etc/chromium, /etc/opt/edge, /usr/lib/mozilla and
# /usr/lib64/mozilla), naming /usr/bin/misterdownloadmanager-nmhost. The package owns them, so
# uninstalling it removes them (not conffiles: `apt remove` takes them too), and MDM writes none
# in the home folder for a browser that reads them. The AppImage and the tar.gz register per user.
system="$stage/system"
"$tree/usr/bin/misterdownloadmanager-nmhost" --package-manifests "$system"
deb="$stage/deb"
cp -r "$tree" "$deb"
# Debian and Ubuntu's Firefox reads /usr/lib/mozilla; lib64's is Fedora's and openSUSE's.
cp -r "$system/." "$deb/"
rm -rf "$deb/usr/lib64"
mkdir -p "$deb/DEBIAN"
cat > "$deb/DEBIAN/control" <<EOF
Package: misterdownloadmanager
Version: $version
Architecture: $deb_arch
Maintainer: Mister Download Manager <https://github.com/anefzaoui/misterdownloadmanager>
Depends: libc6 (>= $glibc), libgtk-3-0, libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, ca-certificates
Section: net
Priority: optional
Homepage: https://github.com/anefzaoui/misterdownloadmanager
Description: Download manager
 Speeds up downloads with several connections at once, resumes them, catches them from the
 browser, and downloads videos and torrents.
EOF
dpkg-deb --root-owner-group --build "$deb" "$dist/$name.deb"

# .rpm, from the same tree (nothing is compiled by rpmbuild).
rpm="$stage/rpm"
mkdir -p "$rpm"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
cat > "$rpm/SPECS/mdm.spec" <<EOF
Name: misterdownloadmanager
Version: $version
Release: 1
Summary: Download manager
License: GPL-3.0-or-later
URL: https://github.com/anefzaoui/misterdownloadmanager
Requires: glibc >= $glibc, gtk3, mesa-libGL, mesa-libEGL, libxkbcommon, libxkbcommon-x11, ca-certificates
AutoReqProv: no

%description
Speeds up downloads with several connections at once, resumes them, catches them from the
browser, and downloads videos and torrents.

%install
cp -a "$tree/." %{buildroot}/
cp -a "$system/." %{buildroot}/

%files
/usr/bin/misterdownloadmanager
/usr/bin/misterdownloadmanager-nmhost
/usr/bin/misterdownloadmanager-cli
/usr/bin/misterdownloadmanager-mcp
/usr/share/applications/misterdownloadmanager.desktop
/usr/share/icons/hicolor/256x256/apps/misterdownloadmanager.png
/usr/share/misterdownloadmanager
/usr/share/doc/misterdownloadmanager
$(cd "$system" && find . -mindepth 1 -type d | sed 's|^\.||' | grep -Ev '^/(etc|etc/opt|usr|usr/lib|usr/lib64)$' | sort | sed 's|^|%dir |')
$(cd "$system" && find . -type f | sed 's|^\.||' | sort)
EOF
rpmbuild --define "_topdir $rpm" --define "_build_id_links none" -bb "$rpm/SPECS/mdm.spec"
cp "$rpm"/RPMS/$pkg_arch/*.rpm "$dist/$name.rpm"

# AppImage: the same tree as an AppDir.
appdir="$stage/AppDir"
cp -r "$tree" "$appdir"
ln -s usr/bin/misterdownloadmanager "$appdir/AppRun"
cp "$tree/usr/share/applications/misterdownloadmanager.desktop" "$appdir/"
cp "$tree/usr/share/icons/hicolor/256x256/apps/misterdownloadmanager.png" "$appdir/"
# appimagetool and the runtime it puts at the front of every AppImage, each at a fixed release and
# checked by its sha256: their `continuous` tags move with every upstream commit, so the same MDM
# tag could otherwise be packaged with a different tool. A new version is a new pair of hashes here
# (GitHub shows each file's sha256 on its release page).
appimagetool_tag=1.9.1
runtime_tag=20251108
case "$pkg_arch" in
  x86_64)
    appimagetool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64)
    appimagetool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
esac
tool="$stage/appimagetool"
runtime="$stage/runtime"
python3 "$root/tools/installer/fetch_release.py" AppImage/appimagetool "$appimagetool_tag" \
  "appimagetool-$pkg_arch.AppImage" "$tool" "$appimagetool_sha"
python3 "$root/tools/installer/fetch_release.py" AppImage/type2-runtime "$runtime_tag" \
  "runtime-$pkg_arch" "$runtime" "$runtime_sha"
chmod +x "$tool"
# Update information (#24): AppImageLauncher, Gear Lever and AppImageUpdate read it from the
# AppImage, find the latest release's .zsync and download only the parts that changed. MDM's own
# Update now doesn't need it. The pattern also matches the names up to 0.2.4, with "-experimental".
update="gh-releases-zsync|anefzaoui|misterdownloadmanager|latest|MisterDownloadManager-*-linux-$arch*.AppImage.zsync"
# No FUSE on CI runners: the tool runs from its own extracted copy.
# zstd at its strongest level (#25; unpacking is as fast): the tool's 128K blocks stay, as 1M
# blocks make the AppImage start slower (AppImage/appimagetool#64).
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$pkg_arch "$tool" -u "$update" --runtime-file "$runtime" \
  --mksquashfs-opt -Xcompression-level --mksquashfs-opt 22 "$appdir" "$dist/$name.AppImage"
# appimagetool writes the .zsync itself (beside the AppImage, or in the current folder); with
# one that doesn't, zsyncmake makes it.
[ -f "$name.AppImage.zsync" ] && mv "$name.AppImage.zsync" "$dist/"
if [ ! -f "$dist/$name.AppImage.zsync" ]; then
  (cd "$dist" && zsyncmake -u "$name.AppImage" -o "$name.AppImage.zsync" "$name.AppImage")
fi

rm -rf "$stage"
(cd "$dist" && sha256sum "$name".* > SHA256SUMS-linux.txt)
ls -l "$dist"
