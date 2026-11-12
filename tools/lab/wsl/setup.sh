#!/usr/bin/env bash
# mdm-lab: a WSL2 distribution that builds MDM for Linux at the PC's full speed and screenshots
# its windows on a virtual screen (Xvfb) that never shows on Windows. It replaces the VirtualBox
# machines for builds and window shots (those run about 3x slower under Hyper-V); README.md says
# what still needs them.
#
# From Windows (PowerShell), once:
#   wsl --install -d Ubuntu-24.04 --name mdm-lab --location C:\mdm-wsl --no-launch
#   wsl -d mdm-lab -u root -- bash /mnt/c/<checkout>/tools/lab/wsl/setup.sh
#   wsl --terminate mdm-lab        # wsl.conf (no Windows programs, user lab) applies at the next start
# Running it again only adds what's missing and updates the checkout. It touches nothing outside
# mdm-lab: never %USERPROFILE%\.wslconfig, never another distribution.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root: wsl -d mdm-lab -u root -- bash $0" >&2; exit 2; }
user=lab
repo_url=https://github.com/anefzaoui/misterdownloadmanager.git
export DEBIAN_FRONTEND=noninteractive

# --- The distribution's own settings (per distribution; .wslconfig is global and stays as it is).
# Interop off: nothing in here can start a Windows program (a browser, explorer.exe), and
# Windows' PATH stays out of builds. WSLg can't be turned off per distribution (guiApplications
# is a .wslconfig key), so env.sh keeps every program off its display, sound and runtime folder.
cat > /etc/wsl.conf <<'EOF'
# mdm-lab: MDM's build and screenshot machine (tools/lab/wsl/setup.sh).
[boot]
systemd=true

[interop]
enabled=false
appendWindowsPath=false

[user]
default=lab
EOF
id "$user" >/dev/null 2>&1 || useradd -m -s /bin/bash -G sudo "$user"
echo "$user ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/$user
chmod 440 /etc/sudoers.d/$user

# --- Packages: the build's libraries (the same as ci.yml, release.yml and the VMs' setup.sh),
# the helpers MDM drives (ffmpeg, 7-Zip: their windows show what is found), the virtual screen
# with a window manager that frames windows as a desktop does (Metacity: GNOME's own frames,
# without a compositor, so a capture reads the screen as drawn), capture and input tools, and
# the fonts of an Ubuntu desktop with every script the languages need.
apt-get update -q
apt-get install -y -q --no-install-recommends \
  build-essential pkg-config git curl ca-certificates python3 python3-pil \
  libgtk-3-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev cmake libclang-dev \
  libgl1-mesa-dri libglx-mesa0 mesa-utils mesa-vulkan-drivers vulkan-tools \
  xvfb xauth x11-utils x11-apps x11-xserver-utils xdotool wmctrl imagemagick metacity dbus-x11 \
  adwaita-icon-theme gnome-themes-extra yaru-theme-gtk yaru-theme-icon \
  ffmpeg p7zip-full \
  fontconfig fonts-ubuntu fonts-dejavu-core fonts-freefont-ttf fonts-liberation \
  fonts-noto-core fonts-noto-ui-core fonts-noto-mono fonts-noto-cjk fonts-noto-color-emoji \
  fonts-kacst fonts-indic fonts-khmeros-core fonts-lao fonts-thai-tlwg fonts-sil-abyssinica
fc-cache -f >/dev/null

# --- As lab: Rust, the lab's cargo profile, the checkout on the distribution's own disk (ext4:
# building from /mnt/c or /mnt/d goes through 9P, many times slower).
here=$(cd "$(dirname "$0")" && pwd)
sudo -u "$user" -H bash -s "$repo_url" "$here" <<'USER'
set -euo pipefail
repo_url=$1; here=$2
if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
. "$HOME/.cargo/env"
rustup update stable >/dev/null
# The VMs' profile (tools/lab/linux-vm/setup.sh): release without the shipping build's fat LTO,
# incremental, so a branch switch or a one-line change recompiles only what changed.
mkdir -p "$HOME/.cargo"
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
mkdir -p "$HOME/src" "$HOME/lab/bin"
[ -d "$HOME/src/mdm/.git" ] || git clone -q "$repo_url" "$HOME/src/mdm"
git -C "$HOME/src/mdm" fetch -q origin
# The lab scripts, copied in so they run from ext4 (refreshed by every setup run).
mkdir -p "$HOME/lab/tools"
cp "$here"/*.sh "$here"/*.py "$here/../linux-vm/pixdiff.py" "$HOME/lab/tools/"
# The toolchain rust-toolchain.toml names, installed now rather than at the first build.
# (Under build.sh's lock: never in the middle of someone's build.)
mkdir -p "$HOME/lab"
(cd "$HOME/src/mdm" && flock "$HOME/lab/build.lock" git checkout -q -f --detach origin/main \
  && rustup show active-toolchain >/dev/null)
echo "lab ready: $(. /etc/os-release; echo "$PRETTY_NAME"), $(rustc --version), $(nproc) cores"
USER
