<#
A build from mdm-lab (WSL2) into a VirtualBox lab machine's ~/lab/bin/<label>/, so a check on a
real desktop (GNOME or KDE portals, the tray, real browsers) never waits for the VM to build.

  tools/lab/wsl/deploy.ps1 <label> [<vm>] [-DryRun]     e.g. deploy.ps1 main mdm-ubuntu

The programs are read straight from the distribution's disk (\\wsl$\mdm-lab\home\lab\lab\bin\<label>)
and copied with VBoxManage guestcontrol into a folder beside the target, then renamed into place
(a running program or script is never overwritten half-way). The VM must be running; nothing else
of it changes. Its account: MDM_VM_USER / MDM_VM_PASSWORD, never printed. -DryRun: what it would copy, and
where, without calling VBoxManage (its guestcontrol copyto has no dry run of its own).
#>
param(
    [Parameter(Position = 0, Mandatory)] [string] $Label,
    [Parameter(Position = 1)] [string] $Vm = 'mdm-ubuntu',
    [string] $Distro = 'mdm-lab',
    [switch] $DryRun
)
$ErrorActionPreference = 'Stop'
$vbox = Join-Path $env:ProgramFiles 'Oracle\VirtualBox\VBoxManage.exe'
if ($Label -notmatch '^[A-Za-z0-9._-]+$') { throw "label: letters, digits, '.', '_' and '-' only" }

# The account, its password only ever in files (account.ps1): never on a command line.
. (Join-Path $PSScriptRoot '..\linux-vm\account.ps1')
$account = Open-VmAccount
$user = $account.User
$auth = $account.Auth
try {

$src = "\\wsl$\$Distro\home\lab\lab\bin\$Label"
if (-not (Test-Path "$src\misterdownloadmanager")) { throw "no build $Label in $Distro (tools/lab/wsl/build.sh)" }

function Invoke-Guest([string] $Command) {
    & $vbox guestcontrol $Vm run --exe /bin/bash @auth --wait-stdout --wait-stderr -- -c $Command
    if ($LASTEXITCODE) { throw "guest command failed ($LASTEXITCODE)" }
}

$files = Get-ChildItem -File $src | ForEach-Object FullName
$incoming = "/home/$user/lab/bin/.incoming-$Label"
$started = Get-Date
if ($DryRun) {
    foreach ($f in Get-ChildItem -File $src) { "  {0,-32} {1,10:N0} bytes" -f $f.Name, $f.Length }
    "dry run: these $($files.Count) files would go to $Vm ~/lab/bin/$Label (through $incoming/); nothing copied"
    return
}
$state = (& $vbox showvminfo $Vm --machinereadable | Select-String '^VMState=').ToString()
if ($state -notmatch 'running') { throw "$Vm is not running ($state)" }
Invoke-Guest "rm -rf '$incoming' && mkdir -p '$incoming'"
& $vbox guestcontrol $Vm copyto @auth --target-directory "$incoming/" @files
if ($LASTEXITCODE) { throw "copy to $Vm failed ($LASTEXITCODE)" }
# The copy keeps no file modes: the programs made runnable, then swapped in whole.
Invoke-Guest "chmod +x '$incoming'/* && rm -rf ~/lab/bin/'$Label' && mv '$incoming' ~/lab/bin/'$Label' && ls -l ~/lab/bin/'$Label'"
$mb = [math]::Round(((Get-ChildItem -File $src | Measure-Object Length -Sum).Sum / 1MB), 1)
"deployed $Label ($mb MB) to $Vm ~/lab/bin/$Label in $([math]::Round(((Get-Date) - $started).TotalSeconds, 1)) s"
} finally {
    Close-VmAccount $account
}
