# Checks the workspace for Linux and macOS from Windows: `cargo clippy -D warnings` for
# x86_64-unknown-linux-gnu and aarch64-apple-darwin, into target/cross. Nothing is linked, so
# no Linux or macOS libraries are needed: Zig (a portable zip from ziglang.org; $env:ZIG, else
# %USERPROFILE%\.mdm-dev\zig-*\zig.exe) compiles the few C dependencies, and pkg-config is a
# stub. The real builds run on GitHub's runners (release.yml).
#
#   tools\dev\check-cross.ps1 [-Targets linux, macos]
param([string[]]$Targets = @("linux", "macos"))
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$zig = if ($env:ZIG) { $env:ZIG } else {
  Get-ChildItem "$env:USERPROFILE\.mdm-dev\zig-*\zig.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
}
if (-not $zig -or -not (Test-Path $zig)) { throw "Zig not found: set `$env:ZIG to zig.exe" }

# Wrappers cargo's C builds call: zig as the compiler (the cc crate's own target flags are off,
# CRATE_CC_NO_DEFAULTS, as zig names the target itself), zig ar, and a pkg-config that finds all.
$wrap = Join-Path $root "target\cross\bin"
New-Item -ItemType Directory -Force $wrap | Out-Null
Set-Content -Encoding ascii "$wrap\cc-linux.cmd" "@`"$zig`" cc -target x86_64-linux-gnu.2.35 %*"
Set-Content -Encoding ascii "$wrap\cc-macos.cmd" "@`"$zig`" cc -target aarch64-macos %*"
Set-Content -Encoding ascii "$wrap\ar.cmd" "@`"$zig`" ar %*"
Set-Content -Encoding ascii "$wrap\pkg-config.cmd" "@exit /b 0"
$env:CRATE_CC_NO_DEFAULTS = "1"
$env:PKG_CONFIG_ALLOW_CROSS = "1"

# rustup and cargo report on stderr: not errors (their exit codes are).
$ErrorActionPreference = "Continue"
$failed = @()
foreach ($t in $Targets) {
  $triple = @{ linux = "x86_64-unknown-linux-gnu"; macos = "aarch64-apple-darwin" }[$t]
  if (-not $triple) { throw "unknown target $t (linux, macos)" }
  $var = $triple -replace "-", "_"
  Set-Item "Env:CC_$var" "$wrap\cc-$t.cmd"
  Set-Item "Env:AR_$var" "$wrap\ar.cmd"
  Set-Item "Env:PKG_CONFIG_$var" "$wrap\pkg-config.cmd"
  & rustup target add $triple | Out-Null
  "== $t ($triple)"
  # Without the engine's browser-like connection (#668): its BoringSSL doesn't cross-compile
  # through Zig here (CMake looks for the target's threads and macOS tools); the runners build it.
  # The other default features are named back.
  & cargo clippy --workspace --all-targets --no-default-features --features mdm-crash/native-symbols --target $triple --target-dir (Join-Path $root "target\cross") -q -- -D warnings
  if ($LASTEXITCODE) { $failed += $t }
}
if ($failed) { throw "failed: $($failed -join ', ')" }
"ok: $($Targets -join ', ')"
