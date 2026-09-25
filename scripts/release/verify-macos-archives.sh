#!/usr/bin/env bash
# Check the macOS release archives that `dist build` wrote to target/distrib:
#   - each archive has the layout the installers expect (<archive name>/<binary>,
#     with no ./ prefix),
#   - its binary has a valid signature with the hardened runtime and the
#     identifier it has always had ("aranet", "aranet-gui"),
#   - when <authority> is given, the signature is from that certificate,
#   - the .sha256 file and the dist manifest both describe the archive on disk.
#
# Usage: scripts/release/verify-macos-archives.sh <target-triple> <dist-manifest.json> [authority]
#   [authority] is e.g. "Developer ID Application: Cameron Rye (ABCDE12345)".
# Used by .github/workflows/release-build-local.yml after signing.
set -euo pipefail

usage="usage: verify-macos-archives.sh <target-triple> <dist-manifest.json> [authority]"
target="${1:?$usage}"
manifest="${2:?$usage}"
authority="${3:-}"

target_dir="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
distrib="$target_dir/distrib"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for pair in aranet-cli:aranet aranet-gui:aranet-gui; do
  package="${pair%%:*}"
  bin="${pair##*:}"
  archive="$package-$target.tar.xz"

  # Check the entry names, not the unpacked tree: the installers unpack with
  # `tar --strip-components 1`, so a ./ prefix (v0.2.0's re-packed archives)
  # unpacks to the same files here but leaves the binary one level too deep there.
  listing="$(tar -tJf "$distrib/$archive")"
  if ! grep -qxF "$package-$target/$bin" <<<"$listing"; then
    echo "error: $archive does not contain $package-$target/$bin (with no ./ prefix)" >&2
    exit 1
  fi
  tar -xJf "$distrib/$archive" -C "$work"
  binary="$work/$package-$target/$bin"
  codesign --verify --strict --verbose=2 "$binary"
  details="$(codesign -dv --verbose=2 "$binary" 2>&1)"
  if ! grep -q 'flags=.*runtime' <<<"$details"; then
    echo "error: $archive: $bin is not signed with the hardened runtime" >&2
    echo "$details" >&2
    exit 1
  fi
  if ! grep -qx "Identifier=$bin" <<<"$details"; then
    echo "error: $archive: $bin has the wrong code-signing identifier" >&2
    echo "$details" >&2
    exit 1
  fi
  if [ -n "$authority" ] && ! grep -qxF "Authority=$authority" <<<"$details"; then
    echo "error: $archive: $bin is not signed by $authority" >&2
    echo "$details" >&2
    exit 1
  fi

  actual="$(shasum -a 256 "$distrib/$archive" | cut -d' ' -f1)"
  recorded="$(cut -d' ' -f1 "$distrib/$archive.sha256")"
  in_manifest="$(jq -r --arg a "$archive" '.artifacts[$a].checksums.sha256' "$manifest")"
  if [ "$actual" != "$recorded" ] || [ "$actual" != "$in_manifest" ]; then
    echo "error: $archive checksum mismatch: file $actual, .sha256 $recorded, manifest $in_manifest" >&2
    exit 1
  fi
  echo "ok: $archive ($actual)"
done
