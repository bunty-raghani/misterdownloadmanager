<#
The README's, docs' and site's Linux screenshots, from the Windows host: docs-shots.sh runs in a
lab VM (GNOME in mdm-ubuntu; KDE Plasma, Cinnamon and Xfce in its clones), its PNGs come back into
target\linux-shots\<vm>\, and -Publish writes them as lossless WebP where the docs and the site
show them. The host's own screen, keyboard and mouse are never used.

  tools\lab\wsl\deploy.ps1 main mdm-ubuntu                      # the build, from mdm-lab
  tools\lab\linux-vm\docs-shots-host.ps1 main                   # every shot, light and dark
  tools\lab\linux-vm\docs-shots-host.ps1 main -Only tray -TrayAt "1166 14"
  tools\lab\linux-vm\docs-shots-host.ps1 main -Publish          # into docs/images and the site
  tools\lab\linux-vm\docs-shots-host.ps1 main -Vm mdm-kde -Desktop kde -Modes light -Only main -MainAt "40,30" -Publish

What was taken for #186: GNOME (mdm-ubuntu) the whole list with -TrayAt "1153 16"; KDE Plasma,
Cinnamon and Xfce (the clones) their main window with -MainAt "40,30" (Plasma keeps a login
notification at the bottom right). A shot taken while the VM is busy can show it (a far round
trip, a window still opening): look at each one, and take that one again with -Only (the others
stay).

GNOME (-Desktop gnome, the default) publishes docs/images/<shot>-linux.webp (the dark main window
as main-linux-dark.webp) and the site's pair website/assets/screens/main-linux-<light|dark>.webp;
another desktop publishes its main window as docs/images/<desktop>-main-linux.webp.

The tray's shot clicks MDM's icon in the top bar: -TrayAt "x y" is its place on the VM's screen
(a first run without it saves the top bar as topbar.png to find it). The VM's account comes from
MDM_VM_USER / MDM_VM_PASSWORD or the local file deploy.ps1 reads, and is never printed.
#>
param(
    [Parameter(Position = 0, Mandatory)] [string] $Label,
    [string] $Vm = 'mdm-ubuntu',
    [ValidateSet('gnome', 'kde', 'cinnamon', 'xfce')] [string] $Desktop = 'gnome',
    [string] $Modes = 'light,dark',
    [string] $Only = '',
    [string] $TrayAt = '',
    # Where the main window goes ("x,y"), clear of a desktop's lasting notification.
    [string] $MainAt = '',
    [switch] $Publish,
    [switch] $NoShoot
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$vbox = Join-Path $env:ProgramFiles 'Oracle\VirtualBox\VBoxManage.exe'
if ($Label -notmatch '^[A-Za-z0-9._-]+$') { throw "label: letters, digits, '.', '_' and '-' only" }
$user = $env:MDM_VM_USER; $pass = $env:MDM_VM_PASSWORD
$keys = Join-Path $env:USERPROFILE '.mdm-dev\vm-account.txt'
if ((-not $user -or -not $pass) -and (Test-Path $keys)) {
    foreach ($line in Get-Content $keys) {
        if (-not $user -and $line -match '^\s*user:\s*(\S+)') { $user = $Matches[1] }
        if (-not $pass -and $line -match '^\s*password:\s*(\S+)') { $pass = $Matches[1] }
    }
}
if (-not $user -or -not $pass) { throw "no VM account: set MDM_VM_USER and MDM_VM_PASSWORD" }
$auth = @('--username', $user, '--password', $pass)
$local = Join-Path $root "target\linux-shots\$Vm"

if (-not $NoShoot) {
    $state = (& $vbox showvminfo $Vm --machinereadable | Select-String '^VMState=').ToString()
    if ($state -notmatch 'running') { throw "$Vm is not running ($state)" }
    $guestOut = "/home/$user/lab/shots/docs-$Label"
    # The script and the torrent the Add torrent shot opens.
    & $vbox guestcontrol $Vm copyto @auth --target-directory "/home/$user/lab/" (Join-Path $PSScriptRoot 'docs-shots.sh')
    & $vbox guestcontrol $Vm copyto @auth (Join-Path $root 'tools\lab\Open Movie Night.torrent') "/home/$user/lab/docs-shots.torrent"
    if ($LASTEXITCODE) { throw "copy to $Vm failed ($LASTEXITCODE)" }
    # The guest's errors on its standard output: Windows PowerShell stops at a native command's
    # stderr under ErrorActionPreference Stop.
    $env_ = "exec 2>&1; export MDM_SUDO_PASSWORD='$pass' MDM_TRAY_AT='$TrayAt' MDM_MAIN_AT='$MainAt'; "
    & $vbox guestcontrol $Vm run --exe /bin/bash @auth --timeout 1200000 --wait-stdout --wait-stderr -- -c `
        ($env_ + "rm -rf '$guestOut'; bash ~/lab/docs-shots.sh '$Label' '$guestOut' '$Modes' '$Only'")
    if ($LASTEXITCODE) { throw "docs-shots.sh failed ($LASTEXITCODE)" }
    # A run's modes replace their folders; a run with -Only replaces those shots only.
    foreach ($m in $Modes -split ',') {
        if (-not $Only) { Remove-Item -Recurse -Force "$local\$m" -ErrorAction SilentlyContinue }
        foreach ($n in ($Only -split ',' | Where-Object { $_ })) { Remove-Item -Force "$local\$m\$n.png" -ErrorAction SilentlyContinue }
    }
    New-Item -ItemType Directory -Force $local | Out-Null
    $ErrorActionPreference = 'Continue'
    foreach ($m in $Modes -split ',') {
        & $vbox guestcontrol $Vm copyfrom @auth --recursive --target-directory "$local\" "$guestOut/$m" 2>&1 | Out-Null
    }
    $ErrorActionPreference = 'Stop'
    Get-ChildItem -Recurse -File $local | ForEach-Object { "  {0}\{1}" -f $_.Directory.Name, $_.Name }
}

if ($Publish) {
    # Lossless WebP (Pillow, method 6): the same pixels as the PNG, opaque.
    $py = @'
import sys
from pathlib import Path
from PIL import Image
local, root, desktop = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
docs, site = root / "docs" / "images", root / "website" / "assets" / "screens"
jobs = []
if desktop == "gnome":
    for name in ["main", "add-url", "progress", "options", "scheduler", "add-torrent", "browser-offer", "tray"]:
        jobs.append((local / "light" / f"{name}.png", docs / f"{name}-linux.webp"))
    jobs.append((local / "dark" / "main.png", docs / "main-linux-dark.webp"))
    # The site shows the main window only (download.html); make.py check fails on a picture
    # nothing shows.
    for mode in ["light", "dark"]:
        jobs.append((local / mode / "main.png", site / f"main-linux-{mode}.webp"))
else:
    jobs.append((local / "light" / "main.png", docs / f"{desktop}-main-linux.webp"))
for src, dst in jobs:
    if not src.exists():
        print(f"  {dst.name}: no {src.parent.name}/{src.name}")
        continue
    im = Image.open(src).convert("RGB")
    im.save(dst, "WEBP", lossless=True, quality=100, method=6)
    kb = dst.stat().st_size / 1024
    print(f"  {dst.relative_to(root).as_posix()}  {im.width}x{im.height}  {kb:.0f} KB" + ("  OVER 300 KB" if kb > 300 else ""))
'@
    $py | python - $local $root $Desktop
}
