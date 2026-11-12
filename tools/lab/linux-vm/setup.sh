#!/usr/bin/env bash
# The Linux test machine: an Ubuntu 24.04 desktop (GNOME on Xorg) that builds and runs MDM, with
# the browsers for the extension and AT-SPI automation for the windows. Run it inside the machine
# as its desktop user (it asks sudo for the system parts); running it again only adds what's
# missing and updates the checkout.
#
#   bash setup.sh [--no-build] [--branch <name>]
#
# From a Windows host, vm.ps1 next to this file copies it in and runs it.
set -euo pipefail

build=1
branch=main
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) build=0 ;;
    --branch) branch="$2"; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

repo_url=https://github.com/anefzaoui/misterdownloadmanager.git
checkout="$HOME/mdm"
# sudo_ takes the password from MDM_SUDO_PASSWORD when set (through stdin, so never pipe into it).
sudo_() { if [ -n "${MDM_SUDO_PASSWORD:-}" ]; then echo "$MDM_SUDO_PASSWORD" | sudo -S -p '' "$@"; else sudo "$@"; fi; }
export DEBIAN_FRONTEND=noninteractive

# --- The desktop: GNOME on Xorg (VirtualBox's display stays on the splash under Wayland), signed
# in automatically, never locked or blanked, so screenshots and automation always see it.
tmp=$(mktemp)
cat >"$tmp" <<EOF
[daemon]
WaylandEnable=false
AutomaticLoginEnable=true
AutomaticLogin=$USER

[security]

[xdmcp]

[chooser]

[debug]
EOF
sudo_ install -m 644 "$tmp" /etc/gdm3/custom.conf
rm -f "$tmp"
mkdir -p "$HOME/.config"
echo yes > "$HOME/.config/gnome-initial-setup-done"
if [ -S "/run/user/$(id -u)/bus" ]; then
  export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
  gsettings set org.gnome.desktop.session idle-delay 0
  gsettings set org.gnome.desktop.screensaver lock-enabled false
  gsettings set org.gnome.desktop.screensaver idle-activation-enabled false
  # AT-SPI for every toolkit: agent-desktop and Orca read the windows through it.
  gsettings set org.gnome.desktop.interface toolkit-accessibility true
fi
# A login keyring with an empty password (auto-login unlocks nothing): gnome-keyring keeps it
# unencrypted and unlocked, so a browser asking the Secret Service (Chrome's "Safe Storage")
# never stops a test with "Choose a password for your new keyring". Test browsers also run with
# --password-store=basic. MDM itself doesn't use the Secret Service.
keyrings="$HOME/.local/share/keyrings"
mkdir -p "$keyrings" && chmod 700 "$keyrings"
if [ ! -f "$keyrings/login.keyring" ]; then
  printf '[keyring]\ndisplay-name=Login\nctime=0\nmtime=0\nlock-on-idle=false\nlock-after=false\n' > "$keyrings/login.keyring"
  chmod 600 "$keyrings/login.keyring"
  echo login > "$keyrings/default"
  systemctl --user restart gnome-keyring-daemon.service 2>/dev/null || true
fi

# --- Packages: the build's libraries (the same as ci.yml and release.yml), the packaging tools,
# the helpers MDM drives (ffmpeg, 7-Zip, gallery-dl), the AppImage runtime (FUSE 2), and the
# automation and screen-reader tools.
sudo_ apt-get update -q
sudo_ apt-get install -y -q \
  build-essential pkg-config git curl ca-certificates python3 python3-pip python3-pil \
  libgtk-3-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev cmake libclang-dev \
  rpm zsync libfuse2t64 \
  ffmpeg p7zip-full gallery-dl \
  at-spi2-core python3-pyatspi orca xdotool wmctrl x11-utils mesa-utils imagemagick \
  gnome-shell-extension-appindicator

# yt-dlp: the distribution's copy falls behind the sites, so the project's own build.
if ! command -v yt-dlp >/dev/null || [ "$(readlink -f "$(command -v yt-dlp)")" != /usr/local/bin/yt-dlp ]; then
  sudo_ curl -fsSL -o /usr/local/bin/yt-dlp https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp
  sudo_ chmod 755 /usr/local/bin/yt-dlp
fi

# --- Browsers: Firefox as Ubuntu ships it (Snap), Chromium (Snap) and Google Chrome (.deb), each
# with its own native messaging folder.
# The Snap store's downloads sometimes stall ("download too slow"): a few tries each.
snap_install() {
  snap list "$1" >/dev/null 2>&1 && return 0
  for _ in 1 2 3; do sudo_ snap install "$1" && return 0; sleep 10; done
  echo "warning: snap $1 not installed; run setup again later" >&2
}
snap_install firefox
snap_install chromium
if ! command -v google-chrome >/dev/null; then
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/chrome.deb" https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
  sudo_ apt-get install -y -q "$tmp/chrome.deb"
  rm -rf "$tmp"
fi

# --- Node LTS and agent-desktop (UI automation through the accessibility tree).
if ! command -v node >/dev/null; then
  curl -fsSL -o /tmp/nodesource.sh https://deb.nodesource.com/setup_lts.x
  sudo_ bash /tmp/nodesource.sh
  rm -f /tmp/nodesource.sh
  sudo_ apt-get install -y -q nodejs
fi
command -v agent-desktop >/dev/null || sudo_ npm install -g agent-desktop

# --- Rust (stable; rust-toolchain.toml picks the components).
if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
# shellcheck disable=SC1091
. "$HOME/.cargo/env"
rustup update stable
# The lab's own profile for functional checks (build.sh): release without the shipping build's
# fat LTO, incremental, so switching branches recompiles only what changed. This machine only.
if ! grep -q 'profile.vmtest' "$HOME/.cargo/config.toml" 2>/dev/null; then
  cat >> "$HOME/.cargo/config.toml" <<'EOF'

[profile.vmtest]
inherits = "release"
lto = false
codegen-units = 256
incremental = true
debug = false
opt-level = 2

[profile.vmtest.package."*"]
opt-level = 3
EOF
fi

# --- The checkout and the build (its target folder stays in the machine).
if [ ! -d "$checkout/.git" ]; then
  git clone "$repo_url" "$checkout"
fi
git -C "$checkout" fetch -q origin
git -C "$checkout" checkout -q "$branch"
git -C "$checkout" pull -q --ff-only || true
if [ "$build" = 1 ]; then
  bash "$(dirname "$0")/build.sh" "$branch"
fi

echo "lab ready: $(lsb_release -ds), $(rustc --version), node $(node --version), $(agent-desktop --version 2>/dev/null || echo 'agent-desktop ?')"
