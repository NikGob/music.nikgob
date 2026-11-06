#!/usr/bin/env bash
# Builds the flake's package and packs its whole closure into
# releases/music.nikgob-<version>-nixos-x86_64.tar.gz, for NixOS machines that
# cannot fetch this (private) repository: the archive's install.sh imports
# the closure into the Nix store and adds the package to the user's profile.
#
# Needs Nix with the nix-command and flakes features. The flake is built
# from a copy of the files it uses, so this works from a git worktree whose
# metadata Linux cannot read (a Windows checkout under WSL) and never copies
# a target directory into the store. flake.lock is written back when Nix
# creates or updates it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p}' "$root/Cargo.toml")"
[ -n "$version" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
out="$root/releases"
mkdir -p "$out"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
src="$stage/src"
mkdir -p "$src"
for f in flake.nix flake.lock Cargo.toml Cargo.lock assets crates packaging vendor; do
  [ -e "$root/$f" ] && cp -r "$root/$f" "$src/"
done

echo "music.nikgob $version: nix build"
nix --extra-experimental-features "nix-command flakes" build "path:$src#default" --out-link "$stage/result" --print-build-logs
cmp -s "$src/flake.lock" "$root/flake.lock" 2>/dev/null || cp "$src/flake.lock" "$root/flake.lock"
store="$(readlink -f "$stage/result")"

name="music.nikgob-$version-nixos-x86_64"
pack="$stage/$name"
mkdir -p "$pack"
# shellcheck disable=SC2046
nix-store --export $(nix-store --query --requisites "$store") > "$pack/closure.nar"
echo "$store" > "$pack/store-path"
cat > "$pack/install.sh" <<'EOF'
#!/bin/sh
# Imports music.nikgob into the Nix store and adds it to your profile.
# Importing needs root (the closure is not signed by a key Nix trusts), so
# that one step runs under sudo when you are not root.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
store="$(cat "$here/store-path")"
if [ "$(id -u)" = 0 ]; then
  nix-store --import < "$here/closure.nar" > /dev/null
else
  sudo nix-store --import < "$here/closure.nar" > /dev/null
fi
if [ "${1:-}" = "--no-profile" ]; then
  :
elif nix profile add "$store" 2>/dev/null || nix profile install "$store" 2>/dev/null; then
  echo "music.nikgob is in your profile: run nikgob, or find it in the menu."
else
  nix-env --install "$store"
  echo "music.nikgob is in your profile: run nikgob, or find it in the menu."
fi
cat <<MSG
For the whole system instead (non-flake configuration.nix):
  environment.systemPackages = [ (builtins.storePath "$store") ];
The store path stays until garbage collection unless a profile or the
system refers to it.
MSG
EOF
chmod 755 "$pack/install.sh"
cat > "$pack/README" <<EOF
music.nikgob $version for NixOS (x86_64), built from the flake at nixpkgs
$(sed -n 's/.*"rev": "\([0-9a-f]*\)".*/\1/p' "$src/flake.lock" | head -1).

  ./install.sh               import it and add it to your profile
  ./install.sh --no-profile  only import it (then use the store path below)

Store path: $store
EOF
tar -C "$stage" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"
echo "  $out/$name.tar.gz ($(du -h "$out/$name.tar.gz" | cut -f1))"
