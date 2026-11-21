---
name: Windows, Mac and Linux
title: Download manager for Windows and Linux, with macOS builds
headline: Made for Windows and Linux. On a Mac too.
line: Stable on Windows 10 and 11, installed or portable, and on Linux; macOS builds to try with every release.
lede: MDM is built for 64-bit Windows 10 and 11, as an installer, a portable copy or a plain zip, and for Linux, checked on four desktops. Experimental builds for macOS come with every release too.
description: Mister Download Manager runs on Windows 10 and 11, installed or portable, with Linux (AppImage, .deb, .rpm, .tar.gz) and macOS (.dmg) builds.
group: app
order: 340
since: 0.1.0
shot: /assets/screens/main-light.webp
shot_alt: The main window on Windows 11: the toolbar, the categories on the left and the download list on the right
where: The download page: pick your system's file
help: getting-started.md#choose-a-download
---

## Windows: three ways to have it

On 64-bit Windows 10 and 11, MDM is the stable, recommended build. The [download
page](/download.html) offers three files:

- **The installer** (`setup.exe`), the one to pick unless you have a reason not to. It installs
  for you alone without administrator rights, or for everyone, and can set up browser
  integration and open `.torrent` files and magnet links with MDM.
- **The portable copy** (`portable.zip`), which runs from any folder.
- **The plain zip** (`x64.zip`), the same programs without an installer.

**Help → Check for updates → Update now** downloads a new version, checks its signature, puts it
in place and restarts, for the setup and both zips.

## Windows on ARM and 32-bit Windows

From the version after 0.5.2, the three files come for two more kinds of PC, with the processor
in their names: **Windows on ARM** (`setup-arm64.exe`, `portable-arm64.zip`, `arm64.zip`), for
PCs with a Snapdragon or another ARM processor, which then run MDM without Windows translating it;
and **32-bit Windows 10** (`setup-x86.exe`, `portable-x86.zip`, `x86.zip`). **Update now** keeps
each copy on its own processor's files. MDM downloads yt-dlp's own build for each, and ffmpeg's
for Windows on ARM; no ffmpeg is built for 32-bit Windows that MDM can check, so there you choose
your own in **Helpers...** for the best video qualities. Linux gets its four files for 64-bit
ARM computers too (`linux-arm64`).

## Portable: everything in one folder

Unzip the portable copy anywhere, a USB stick included. The download list, the settings, the
video helpers and torrent data stay in a `data` folder next to the program, and nothing is
written to the registry. Two things need the installer instead: starting with Windows, and
taking downloads from the browser, since both go through the registry. [Carry MDM on a USB
stick](guide-usb-stick.md) shows the set-up.

## Mac and Linux: what you get

For Linux on x86-64 (and ARM64 from the version after 0.5.2): an AppImage for most
distributions, a `.deb` for Ubuntu, Debian, Mint and Pop!_OS, an `.rpm` for Fedora, openSUSE and
RHEL, and a `.tar.gz` of the plain programs. For macOS 11 and later, on Apple Silicon or Intel,
`.dmg` files still marked `-experimental`. Please [tell us how it went](https://github.com/anefzaoui/misterdownloadmanager/issues/new/choose), good
or bad.

There, MDM connects itself to your browsers at each start, has its tray icon, follows the
system's light or dark look and uses the system's fonts and ffmpeg. The Mac app isn't notarized
by Apple yet, so macOS asks before its first start: right-click it and choose **Open** (on
macOS 15 and later, **System Settings → Privacy & Security → Open Anyway**). A `.deb` or `.rpm`
update opens in the system's installer. A few parts work only on Windows, and the help says so
where it matters.

## Linux, checked on four desktops

Linux isn't experimental any more, since version 0.5.0: every change is checked on a Linux
desktop as well as on Windows. GNOME, KDE Plasma, Xfce and Cinnamon were checked window by window: the tray icon and
its menu, **Minimize to the notification area**, Dark Mode support following the desktop, and the
browsers handing downloads over (Ubuntu's Snap Firefox, Google Chrome and Brave). A Wayland
session runs MDM through XWayland, so menus open under their titles, on tiling compositors such
as sway too. The `.deb` and `.rpm` were installed in clean Ubuntu 24.04, Debian 12 and Fedora 45
systems and bring everything MDM needs; from the AppImage or the `.tar.gz`, a missing library is
named at start with the command that installs it. **Update now** asks the package manager which
package MDM is.

![MDM's main window on Ubuntu 24.04's GNOME desktop: the categories and a list of downloads, each with the desktop's icon for its type](images/main-linux.webp)
*On Ubuntu's GNOME desktop, with the desktop's own file icons.*

What's left: the Flatpak Firefox isn't checked yet; screen readers (Orca) don't read MDM's
windows on Linux yet; and native Wayland windows can't place menus, so XWayland stays the
default. macOS stays experimental: its builds are made and tested by GitHub Actions, not yet on a
Mac of ours.

## Smaller Mac downloads, lighter Linux updates

<!--@coming 0.5-->

The Mac gets two smaller downloads, one for Apple Silicon and one for Intel, instead of one
universal `.dmg` of 80 MB. From 0.2.4 or older, update the Mac once by hand: **Download** in
**Check for updates** opens the release, where you pick your Mac's file. The AppImage can update
through AppImageLauncher, Gear Lever and AppImageUpdate, which fetch only the parts that changed.
Linux can draw with Vulkan and macOS with Metal, from **View → Draw with**, while OpenGL stays the
default there. In a Wayland session, MDM runs through XWayland so its menus open where you click.
