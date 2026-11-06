#!/usr/bin/env bash
# Builds the Linux release of music.nikgob and packs it into releases/:
#
#   music.nikgob-<version>-linux-x86_64.tar.gz    the binary, its .desktop entry,
#                                                 the icon and a README
#   music.nikgob-<version>-1-x86_64.pkg.tar.zst   the pacman package (makepkg)
#
# and, with --nixos, the NixOS closure (scripts/release-nixos.sh).
#
# Run it on Arch Linux (the WSL distro does): makepkg is Arch's. The version
# is the workspace's in Cargo.toml. CARGO_TARGET_DIR is honoured, so a build
# kept on the Linux file system is reused; CARGO_BUILD_JOBS defaults to 4 and
# cargo runs at the lowest priority, so the machine stays usable meanwhile.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
nixos=0
for arg in "$@"; do
  case "$arg" in
    --nixos) nixos=1 ;;
    *) echo "usage: $0 [--nixos]" >&2; exit 2 ;;
  esac
done

# The version under [workspace.package].
version="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p}' "$root/Cargo.toml")"
[ -n "$version" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
target="${CARGO_TARGET_DIR:-$root/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
out="$root/releases"
mkdir -p "$out"

echo "music.nikgob $version: building"
(cd "$root" && nice -n 19 cargo build --release --locked -p nikgob)
bin="$target/release/nikgob"
[ -x "$bin" ] || { echo "no binary at $bin" >&2; exit 1; }

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

# The tarball: unpack anywhere and run ./nikgob, or copy the files where the
# README says.
name="music.nikgob-$version-linux-x86_64"
mkdir -p "$stage/$name"
install -m755 "$bin" "$stage/$name/nikgob"
strip --strip-unneeded "$stage/$name/nikgob"
install -m644 "$root/packaging/music.nikgob.desktop" "$stage/$name/"
install -m644 "$root/packaging/icons/music.nikgob-256.png" "$stage/$name/music.nikgob.png"
cat > "$stage/$name/README" <<EOF
music.nikgob $version for Linux (x86_64)

Run ./nikgob. To have it in the application menu, copy
  nikgob                to ~/.local/bin/
  music.nikgob.desktop  to ~/.local/share/applications/
  music.nikgob.png      to ~/.local/share/icons/hicolor/256x256/apps/
(on Arch, the .pkg.tar.zst of the same release does that system-wide).

Runtime packages it needs:
  Arch:          gtk3 webkit2gtk-4.1 alsa-lib openssl wayland libxkbcommon
                 libglvnd libx11 libxcursor libxi libxrandr
  Debian/Ubuntu: libgtk-3-0 libwebkit2gtk-4.1-0 libasound2 libssl3
                 libwayland-client0 libxkbcommon0 libegl1 libgl1 libx11-6
                 libxcursor1 libxi6 libxrandr2
  Fedora:        gtk3 webkit2gtk4.1 alsa-lib openssl-libs libwayland-client
                 libxkbcommon libglvnd-egl libX11 libXcursor libXi libXrandr

Sound goes through PulseAudio or PipeWire's PulseAudio service, ALSA
without either. Optional: a Secret Service (GNOME Keyring, KWallet,
KeePassXC) keeps the Spotify sign-in between runs; ffmpeg turns fake
lossless files into MP3; curl reaches Genius lyrics past its bot check;
xdg-utils opens links and folders.
EOF
tar -C "$stage" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"
echo "  $out/$name.tar.gz"

# The pacman package of the same binary.
pkg="$stage/pkg"
mkdir -p "$pkg"
pkgver="${version//-/_}"
sed "s/^pkgver=.*/pkgver=$pkgver/" "$root/packaging/arch/PKGBUILD" > "$pkg/PKGBUILD"
cp "$stage/$name/nikgob" "$root/packaging/music.nikgob.desktop" "$pkg/"
cp "$root"/packaging/icons/music.nikgob-*.png "$pkg/"
(
  cd "$pkg"
  # A repack of a finished binary: its build dependencies are not needed.
  PKGDEST="$out" PKGEXT='.pkg.tar.zst' SRCDEST="$pkg" BUILDDIR="$pkg/build" \
    makepkg --force --nodeps --noconfirm >/dev/null
)
echo "  $out/music.nikgob-$pkgver-1-x86_64.pkg.tar.zst"

if [ "$nixos" = 1 ]; then
  "$root/scripts/release-nixos.sh"
fi
