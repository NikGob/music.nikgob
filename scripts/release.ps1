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
$env:RUSTFLAGS = '-C target-feature=+crt-static'
try {
    cargo build --release -p nikgob --target $target --target-dir target\dist
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
} finally {
    Remove-Item Env:RUSTFLAGS
}

New-Item -ItemType Directory -Force releases | Out-Null
Copy-Item "target\dist\$target\release\nikgob.exe" $out -Force
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "releases\music.nikgob-$version.exe ($mb MB)"
