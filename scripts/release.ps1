# Builds the player as one exe to hand out and puts it in releases\
# as music.nikgob-<version>.exe, the version taken from Cargo.toml.
#
# The C runtime is linked in (crt-static), so the exe needs nothing but
# Windows itself. It is built in target\dist so the dev builds keep their
# own artifacts.
#
#   .\scripts\release.ps1          builds the version in Cargo.toml
#   .\scripts\release.ps1 -Force   builds it again over an existing exe

param([switch]$Force)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw 'no version in Cargo.toml' }
$out = Join-Path $root "releases\music.nikgob-$version.exe"
if ((Test-Path $out) -and -not $Force) {
    throw "$out is already there: raise the version in Cargo.toml, or pass -Force"
}

$target = 'x86_64-pc-windows-msvc'
# The paths of this machine stay out of the exe (panic places, backtraces):
# the repo reads as nikgob\, the registry as cargo\, the toolchain as rustup\.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$env:RUSTFLAGS = "-C target-feature=+crt-static --remap-path-prefix=$root=nikgob --remap-path-prefix=$cargoHome=cargo --remap-path-prefix=$rustupHome=rustup"
# A handout build shows its version at the foot of Настройки; the owner's
# own builds say «admin» there.
$env:NIKGOB_RELEASE = '1'
try {
    cargo build --release -p nikgob --target $target --target-dir target\dist
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
} finally {
    Remove-Item Env:RUSTFLAGS
    Remove-Item Env:NIKGOB_RELEASE
}

New-Item -ItemType Directory -Force releases | Out-Null
Copy-Item "target\dist\$target\release\nikgob.exe" $out -Force
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "releases\music.nikgob-$version.exe ($mb MB)"
