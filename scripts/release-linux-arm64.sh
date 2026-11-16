#!/usr/bin/env bash
# Builds nikgobd, the player without a window, for Linux on 64-bit ARM
# (Raspberry Pi 4/5) and packs it into releases/:
#
#   nikgobd-<version>-linux-aarch64.tar.gz   the binary, an example config,
#                                            the systemd user unit, a README
#
# Run it on Arch Linux (the WSL distro does). It cross-compiles with
# cargo-zigbuild: zig is the C compiler and the linker of the C parts
# (SQLite, mimalloc, aws-lc, ring) and links against glibc 2.36, so the
# binary runs on Debian 12 / Raspberry Pi OS bookworm and anything newer.
# cpal always builds its ALSA host, so libasound for aarch64 is needed to
# link: the script takes it once from Debian's arm64 packages into a
# sysroot of its own (cpal's PulseAudio host is pure Rust and needs no
# library).
#
# One-time setup:
#   sudo pacman -S --needed zig curl libarchive xz llvm
#   cargo install --locked cargo-zigbuild
#   rustup target add aarch64-unknown-linux-gnu
#
# The version is the workspace's in Cargo.toml. CARGO_TARGET_DIR is
# honoured; CARGO_BUILD_JOBS defaults to 4 and cargo runs at the lowest
# priority, so the machine stays usable meanwhile.
# NIKGOB_ARM64_SYSROOT  where the ALSA sysroot is kept
#                       (default ~/.cache/nikgob/aarch64-sysroot)
# NIKGOB_ARM64_GLIBC    the oldest glibc the binary runs on (default 2.36)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ "$#" -eq 0 ] || { echo "usage: $0" >&2; exit 2; }

# The version under [workspace.package].
version="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p}' "$root/Cargo.toml")"
[ -n "$version" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
target="${CARGO_TARGET_DIR:-$root/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
out="$root/releases"
mkdir -p "$out"

triple=aarch64-unknown-linux-gnu
glibc="${NIKGOB_ARM64_GLIBC:-2.36}"
suite=bookworm
sysroot="${NIKGOB_ARM64_SYSROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/nikgob/aarch64-sysroot}"
multiarch="$sysroot/usr/lib/aarch64-linux-gnu"

for tool in cargo cargo-zigbuild zig curl bsdtar xz sha256sum; do
  command -v "$tool" >/dev/null || { echo "missing $tool (see the setup at the top of $0)" >&2; exit 1; }
done
if command -v rustup >/dev/null && ! rustup target list --installed | grep -qx "$triple"; then
  echo "missing the Rust target: rustup target add $triple" >&2
  exit 1
fi

# libasound for aarch64, once: libasound2 and libasound2-dev of Debian
# bookworm, checked against the SHA-256 of Debian's package index.
if [ ! -f "$multiarch/pkgconfig/alsa.pc" ]; then
  echo "ALSA for aarch64: Debian $suite's packages into $sysroot"
  mkdir -p "$sysroot"
  work="$(mktemp -d)"
  curl -fsSL "https://deb.debian.org/debian/dists/$suite/main/binary-arm64/Packages.xz" | xz -d > "$work/Packages"
  for pkg in libasound2 libasound2-dev; do
    read -r file sum < <(awk -v p="$pkg" '
      $1 == "Package:" { hit = ($2 == p); file = ""; sum = "" }
      hit && $1 == "Filename:" { file = $2 }
      hit && $1 == "SHA256:" { sum = $2 }
      hit && file != "" && sum != "" { print file, sum; exit }
    ' "$work/Packages") || true
    [ -n "${file:-}" ] && [ -n "${sum:-}" ] || { echo "no $pkg in Debian $suite's index" >&2; rm -rf "$work"; exit 1; }
    curl -fsSL "https://deb.debian.org/debian/$file" -o "$work/$pkg.deb"
    echo "$sum  $work/$pkg.deb" | sha256sum -c --quiet - || { echo "$pkg: the download does not match the index" >&2; rm -rf "$work"; exit 1; }
    # A .deb is an ar archive; the files are in its data.tar.*.
    bsdtar -xOf "$work/$pkg.deb" 'data.tar.*' | bsdtar -xf - -C "$sysroot"
    echo "  $pkg ($(basename "$file"))"
  done
  rm -rf "$work"
  [ -f "$multiarch/pkgconfig/alsa.pc" ] || { echo "no alsa.pc in $multiarch/pkgconfig after unpacking" >&2; exit 1; }
fi

# alsa-sys finds libasound through pkg-config: for the target only, in the
# sysroot (the host's own pkg-config search stays as it is).
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_SYSROOT_DIR_aarch64_unknown_linux_gnu="$sysroot"
export PKG_CONFIG_LIBDIR_aarch64_unknown_linux_gnu="$multiarch/pkgconfig:$sysroot/usr/share/pkgconfig"

# The paths of this machine stay out of the binary (panic places,
# backtraces): the repo reads as nikgob/, the registry as cargo/, the
# toolchain as rustup/.
remap="--remap-path-prefix=$root=nikgob"
remap="$remap --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo"
remap="$remap --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=rustup"

echo "nikgobd $version for $triple (glibc $glibc): building"
(cd "$root" && RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$remap" NIKGOB_RELEASE=1 \
  nice -n 19 cargo zigbuild --release --locked -p nikgobd --target "$triple.$glibc")
bin="$target/$triple/release/nikgobd"
[ -x "$bin" ] || { echo "no binary at $bin" >&2; exit 1; }

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

name="nikgobd-$version-linux-aarch64"
mkdir -p "$stage/$name"
install -m755 "$bin" "$stage/$name/nikgobd"
# The host's strip knows x86 only; LLVM's knows every target.
if command -v llvm-strip >/dev/null; then
  llvm-strip --strip-unneeded "$stage/$name/nikgobd"
else
  echo "  (llvm-strip missing: the binary keeps its symbols)"
fi
install -m644 "$root/packaging/linux/nikgobd.toml" "$stage/$name/"
install -m644 "$root/packaging/linux/nikgobd.service" "$stage/$name/"
{
  echo "nikgobd $version for Linux (aarch64, glibc $glibc or newer)"
  echo
  cat "$root/packaging/linux/README"
} > "$stage/$name/README"

# What it needs of the system, as a check: the newest glibc symbol version
# and the shared libraries.
if command -v llvm-readelf >/dev/null; then
  newest="$(llvm-readelf -V "$stage/$name/nikgobd" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)"
  needed="$(llvm-readelf -d "$stage/$name/nikgobd" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p' | tr '\n' ' ')"
  echo "  needs $newest; links $needed"
fi

tar -C "$stage" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"
echo "  $out/$name.tar.gz"
