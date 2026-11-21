# The lab VM's account, for vm.ps1 and docs-shots-host.ps1 (dot-sourced). The password never goes
# on a command line, on the host or in the VM, where any process listing (`ps -ef`, `pgrep -af`,
# Task Manager's command line column) would show it:
#  - VBoxManage gets it through `--passwordfile`, a file only this user can read, removed when the
#    script ends (Close-VmAccount, from the caller's `finally`);
#  - in the VM, sudo reads it from ~/.config/mdm-lab/sudo (mode 600, written once per VM by
#    Install-VmSudo), which the lab scripts take as MDM_SUDO_PASSWORD with `$(cat …)`, so the
#    command line only ever names the file.
#
# The account comes from MDM_VM_USER and MDM_VM_PASSWORD, or the developer key folder's
# vm-account.txt ("user: <name>" and "password: <word>" lines).

function Open-VmAccount([string] $DefaultUser = 'mdm') {
    $user = if ($env:MDM_VM_USER) { $env:MDM_VM_USER } else { $DefaultUser }
    $pass = $env:MDM_VM_PASSWORD
    $keys = Join-Path $env:USERPROFILE '.mdm-dev\vm-account.txt'
    if (-not $pass -and (Test-Path $keys)) {
        foreach ($line in Get-Content $keys) {
            if (-not $env:MDM_VM_USER -and $line -match '^\s*user:\s*(\S+)') { $user = $Matches[1] }
            if ($line -match '^\s*password:\s*(\S+)') { $pass = $Matches[1] }
        }
    }
    if (-not $pass) { throw "no VM account: set MDM_VM_USER and MDM_VM_PASSWORD" }
    # A run stopped before its `finally` (a killed task, a closed window) left its folder and the
    # password in it: those older than any run takes are removed first.
    Get-ChildItem ([IO.Path]::GetTempPath()) -Directory -Filter 'mdm-vm-*' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt (Get-Date).AddHours(-12) } |
        ForEach-Object { Remove-Item -Recurse -Force $_.FullName -ErrorAction SilentlyContinue }
    # A folder of its own under the user's temp folder (already private to the user), the file
    # named `sudo` so Install-VmSudo can copy it into the VM as it is.
    $dir = Join-Path ([IO.Path]::GetTempPath()) ("mdm-vm-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    $file = Join-Path $dir 'sudo'
    [IO.File]::WriteAllText($file, $pass)
    [pscustomobject]@{
        User = $user
        Dir = $dir
        File = $file
        Auth = @('--username', $user, '--passwordfile', $file)
        Installed = @{}
    }
}

function Close-VmAccount($Account) {
    if ($Account -and $Account.Dir -and (Test-Path $Account.Dir)) {
        Remove-Item -Recurse -Force $Account.Dir -ErrorAction SilentlyContinue
    }
}

# ~/.config/mdm-lab/sudo in the VM `$Vm`, once per script run and VM.
function Install-VmSudo($Account, [string] $Vm, [string] $VBox) {
    if ($Account.Installed[$Vm]) { return }
    $guest = "/home/$($Account.User)/.config/mdm-lab"
    & $VBox guestcontrol $Vm mkdir @($Account.Auth) --parents $guest
    if ($LASTEXITCODE) { throw "could not make $guest in $Vm ($LASTEXITCODE)" }
    & $VBox guestcontrol $Vm copyto @($Account.Auth) --target-directory "$guest/" $Account.File
    if ($LASTEXITCODE) { throw "could not copy the sudo file into $Vm ($LASTEXITCODE)" }
    & $VBox guestcontrol $Vm run --exe /bin/chmod @($Account.Auth) --wait-stdout --wait-stderr -- 600 "$guest/sudo"
    if ($LASTEXITCODE) { throw "could not chmod the sudo file in $Vm ($LASTEXITCODE)" }
    $Account.Installed[$Vm] = $true
}

# What a guest command starts with to have MDM_SUDO_PASSWORD, without the password in it. No
# double quotes: Windows PowerShell passes them to VBoxManage unescaped, which splits `bash -c`'s
# argument (an export's assignment isn't word-split by bash anyway).
$VmSudoEnv = 'export MDM_SUDO_PASSWORD=$(cat ~/.config/mdm-lab/sudo 2>/dev/null); '
