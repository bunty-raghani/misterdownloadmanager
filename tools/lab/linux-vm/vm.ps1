<#
The Linux test machine from the Windows host: a VirtualBox VM (Ubuntu 24.04 desktop, GNOME on
Xorg) that runs headless. Everything goes through VBoxManage, so the host's own keyboard, mouse
and clipboard are never touched; keep the VM's clipboard sharing and drag-and-drop off.

  tools/lab/linux-vm/vm.ps1 configure               # shut down cleanly, apply the lab's hardware
  tools/lab/linux-vm/vm.ps1 start                   # boot headless, wait for the desktop
  tools/lab/linux-vm/vm.ps1 setup [-Branch <name>]  # copy the guest scripts to ~/lab and run setup.sh
                                                    # (long: packages, Rust, the first build)
  tools/lab/linux-vm/vm.ps1 run "<bash command>"    # run in the desktop user's session
  tools/lab/linux-vm/vm.ps1 shot <file.png>         # screenshot of the VM's screen
  tools/lab/linux-vm/vm.ps1 get <guest path> <host folder>
  tools/lab/linux-vm/vm.ps1 put <host file> <guest path>
  tools/lab/linux-vm/vm.ps1 clone <new name> [-Snapshot <name>] [-Cpus 6] [-MemoryMb 10240]
                                                    # a linked clone of a snapshot of $MDM_VM, its own
                                                    # hostname; then `start` it with MDM_VM=<name>
  tools/lab/linux-vm/vm.ps1 deploy <to VM> <label>  # ~/lab/bin/<label> from $MDM_VM to another VM
                                                    # (through a folder on the host), with the guest
                                                    # scripts: a clone tests a build without making it

In the guest (~/lab, from `setup`):
  build.sh <ref> [label]       a branch, PR (pr/<n>) or commit, built with the lab's vmtest profile
                               into ~/lab/bin/<label>/, in a terminal on the lab's desktop
  restart-lab.sh <label>       the lab server and MDM from one build, restarted
  shots.sh <label> <window> <lang> <light|dark>   one window ready for a screenshot (its frame
                               printed); shot-host.sh, on the host, crops VirtualBox's screenshot
                               to it; pixdiff.py compares two folders of shots pixel by pixel
  measure.sh <program> <label> MDM's start-up timings
  streams.mjs, streams-chrome.mjs, run-streams.sh   HLS and DASH players handed to MDM from the
                               Snap Firefox and Google Chrome by the extension's three paths,
                               checked against the lab server's log
  proxy-optin.mjs, chrome-popup.mjs   the extension's popup: the proxy switch and its permission
                               prompt in Firefox; any check of the popup in Chrome
  win.sh (with win-host.sh on the host)   one MDM kept running while several of its windows are
                               shot in turn: start with a language, mode and theme, open, close,
                               switch theme live, real clicks and keys inside the VM
  store-check.mjs              a store package (.xpi, unpacked Chrome zip) installed, its popup, an
                               ordinary download handed to MDM
  chrome-menu.mjs              an HLS stream from Google Chrome's right-click menu (a real right-click
                               in a Chrome window, which headless Chrome can't show)
  desktop.sh                   KDE Plasma, Cinnamon or XFCE beside GNOME in a clone, auto-login to it
  handoff-minimized.sh, queue-minimized.sh   hand-overs and the Scheduler while MDM is minimized
  torrent-menu.sh              a torrent in the list with no network, for the torrent windows
  use-system-helpers.sh        MDM uses the system's yt-dlp, ffmpeg and gallery-dl

Snapshots for clones are taken powered off (`clone` shuts the VM down first when the snapshot
doesn't exist yet): a live snapshot saves the whole memory, and under Hyper-V (VirtualBox's NEM
mode, "VT-x is not available") it stalled with an empty .sav file and had to be killed.

The VM's name, user and password come from MDM_VM, MDM_VM_USER and MDM_VM_PASSWORD (defaults:
mdm-ubuntu and mdm; the password has no default). Commands run with the desktop session's
DISPLAY and D-Bus, so GUI programs open on the VM's screen.
#>
param(
    [Parameter(Position = 0, Mandatory)] [ValidateSet('configure', 'start', 'setup', 'run', 'shot', 'get', 'put', 'clone', 'deploy')] [string] $Action,
    [Parameter(Position = 1)] [string] $Arg1,
    [Parameter(Position = 2)] [string] $Arg2,
    [string] $Branch = 'main',
    [string] $Snapshot = 'base',
    [int] $Cpus = 6,
    [int] $MemoryMb = 10240
)
$ErrorActionPreference = 'Stop'
$vbox = Join-Path $env:ProgramFiles 'Oracle\VirtualBox\VBoxManage.exe'
$vm = if ($env:MDM_VM) { $env:MDM_VM } else { 'mdm-ubuntu' }
# The account, its password only ever in files (account.ps1): never on a command line.
. (Join-Path $PSScriptRoot 'account.ps1')
$account = Open-VmAccount
$user = $account.User
$auth = $account.Auth

function Invoke-Guest([string] $Command, [string] $On = $vm) {
    # The desktop session's display and bus, so GUI programs and gsettings reach it.
    $session = 'export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/$(id -u) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus; ' +
        'export XAUTHORITY=$(ls /run/user/$(id -u)/gdm/Xauthority 2>/dev/null || echo $HOME/.Xauthority); ' +
        $VmSudoEnv + '[ -f ~/.cargo/env ] && . ~/.cargo/env; '
    Install-VmSudo $account $On $vbox
    & $vbox guestcontrol $On run --exe /bin/bash @auth --wait-stdout --wait-stderr -- -c ($session + $Command)
    if ($LASTEXITCODE) { throw "guest command failed ($LASTEXITCODE)" }
}

try {
switch ($Action) {
    'configure' {
        # Settings only change on a powered-off machine: an ACPI shutdown first, as a person would.
        $running = { (& $vbox showvminfo $vm --machinereadable | Select-String '^VMState=').ToString() -match 'running' }
        if (& $running) {
            & $vbox controlvm $vm acpipowerbutton
            for ($i = 0; $i -lt 90 -and (& $running); $i++) { Start-Sleep 2 }
            if (& $running) { throw "$vm did not shut down" }
        }
        # A USB tablet, so a viewer's pointer moves in and out without being captured. 8 CPUs,
        # 12 GB and 3D (VMSVGA): GNOME and MDM's OpenGL drawn by the host's GPU rather than
        # llvmpipe (VirtualBox may run on Hyper-V here, which is slower). Clipboard and
        # drag-and-drop stay off: nothing crosses between the lab and the host.
        & $vbox modifyvm $vm --usb-ohci on --mouse usbtablet --cpus 8 --memory 12288 `
            --graphicscontroller vmsvga --accelerate-3d on --vram 256 `
            --clipboard-mode disabled --drag-and-drop disabled
        if ($LASTEXITCODE) { throw "modifyvm failed ($LASTEXITCODE)" }
    }
    'start' {
        $state = (& $vbox showvminfo $vm --machinereadable | Select-String '^VMState=').ToString()
        if ($state -notmatch 'running') { & $vbox startvm $vm --type headless | Out-Null }
        # Ready when the Guest Additions run (guestcontrol needs them) and the user is signed in.
        for ($i = 0; $i -lt 90; $i++) {
            $level = & $vbox showvminfo $vm --machinereadable | Select-String '^GuestAdditionsRunLevel=[23]'
            $users = & $vbox guestproperty get $vm /VirtualBox/GuestInfo/OS/LoggedInUsersList 2>$null
            if ($level -and "$users" -match "\b$user\b") { break }
            Start-Sleep 2
        }
        Start-Sleep 10
        Invoke-Guest 'loginctl list-sessions --no-legend | grep seat0'
    }
    'setup' {
        Invoke-Guest 'mkdir -p ~/lab'
        # The guest side: setup, the build, the harnesses (everything here but this script).
        $files = Get-ChildItem $PSScriptRoot -File | Where-Object { $_.Extension -in '.sh', '.mjs' -and $_.Name -ne 'shot-host.sh' } | ForEach-Object FullName
        & $vbox guestcontrol $vm copyto @auth --target-directory /home/$user/lab/ @files
        Invoke-Guest "bash ~/lab/setup.sh --branch '$Branch'"
    }
    'clone' {
        # A linked clone shares the snapshot's disk and writes only its own changes: small, and
        # it has the toolchain, the checkout and a warm target folder already.
        $name = $Arg1
        if (-not $name) { throw 'clone: a name for the new VM' }
        $snaps = & $vbox snapshot $vm list --machinereadable 2>$null
        if (-not ($snaps | Select-String "=`"$Snapshot`"")) {
            $running = { (& $vbox showvminfo $vm --machinereadable | Select-String '^VMState=').ToString() -match 'running' }
            if (& $running) {
                & $vbox controlvm $vm acpipowerbutton
                for ($i = 0; $i -lt 90 -and (& $running); $i++) { Start-Sleep 2 }
                if (& $running) { throw "$vm did not shut down" }
            }
            & $vbox snapshot $vm take $Snapshot --description 'Lab base for linked clones (tools/lab/linux-vm)'
        }
        & $vbox clonevm $vm --snapshot $Snapshot --options link --name $name --register
        if ($LASTEXITCODE) { throw "clonevm failed ($LASTEXITCODE)" }
        # The lab's hardware rules, with its own size; clipboard and drag-and-drop stay off.
        & $vbox modifyvm $name --cpus $Cpus --memory $MemoryMb --usb-ohci on --mouse usbtablet `
            --graphicscontroller vmsvga --accelerate-3d on --vram 256 `
            --clipboard-mode disabled --drag-and-drop disabled
        & $vbox startvm $name --type headless | Out-Null
        for ($i = 0; $i -lt 90; $i++) {
            $level = & $vbox showvminfo $name --machinereadable | Select-String '^GuestAdditionsRunLevel=[23]'
            if ($level) { break }
            Start-Sleep 2
        }
        Start-Sleep 10
        Install-VmSudo $account $name $vbox
        & $vbox guestcontrol $name run --exe /bin/bash @auth --wait-stdout --wait-stderr -- -c `
            "sudo -S -p '' hostnamectl set-hostname $name < ~/.config/mdm-lab/sudo && sudo -S -p '' sed -i 's/\b$vm\b/$name/g' /etc/hosts < ~/.config/mdm-lab/sudo && hostname"
    }
    'deploy' {
        # The programs of one build, from the VM that built them to one that only tests (a clone
        # with another desktop): no toolchain or second build needed there. VirtualBox copies
        # between guest and host only, so through a folder on the host, removed afterwards.
        $to = $Arg1; $label = $Arg2
        if (-not $to -or -not $label) { throw 'deploy: <to VM> <label>' }
        $tmp = Join-Path ([IO.Path]::GetTempPath()) "mdm-lab-deploy-$label"
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
        New-Item -ItemType Directory $tmp | Out-Null
        try {
            & $vbox guestcontrol $vm copyfrom @auth --recursive --target-directory "$tmp\\" "/home/$user/lab/bin/$label"
            if ($LASTEXITCODE) { throw "copy from $vm failed ($LASTEXITCODE)" }
            Invoke-Guest "mkdir -p ~/lab/bin && rm -rf ~/lab/bin/$label" -On $to
            & $vbox guestcontrol $to copyto @auth --recursive --target-directory "/home/$user/lab/bin/" (Join-Path $tmp $label)
            if ($LASTEXITCODE) { throw "copy to $to failed ($LASTEXITCODE)" }
            # The guest scripts too, so the clone runs the same harnesses as the lab: copied beside
            # and renamed into place, since bash reads a running script as it goes (a build.sh
            # overwritten mid-build broke at its next line).
            $files = Get-ChildItem $PSScriptRoot -File | Where-Object { $_.Extension -in '.sh', '.mjs', '.py' -and $_.Name -notlike '*-host.sh' } | ForEach-Object FullName
            Invoke-Guest 'rm -rf ~/lab/.incoming && mkdir -p ~/lab/.incoming' -On $to
            & $vbox guestcontrol $to copyto @auth --target-directory /home/$user/lab/.incoming/ @files
            Invoke-Guest 'mv -f ~/lab/.incoming/* ~/lab/ && rmdir ~/lab/.incoming' -On $to
            # The copy keeps no file modes: the programs made runnable again.
            Invoke-Guest "chmod +x ~/lab/bin/$label/* && ls -l ~/lab/bin/$label" -On $to
        } finally {
            Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
        }
    }
    'run' { Invoke-Guest $Arg1 }
    'shot' { & $vbox controlvm $vm screenshotpng ([IO.Path]::GetFullPath($Arg1)) }
    'get' { & $vbox guestcontrol $vm copyfrom @auth --target-directory $Arg2 $Arg1 }
    'put' { & $vbox guestcontrol $vm copyto @auth --target-directory $Arg2 $Arg1 }
}
} finally {
    Close-VmAccount $account
}
