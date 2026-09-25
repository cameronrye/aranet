#!/usr/bin/env bash
# Sign the macOS binaries that `dist build` has just built, inside cargo's target
# directory, so that running `dist build` a second time archives and checksums
# the signed bytes. Used by .github/workflows/release-build-local.yml.
#
# Usage: scripts/release/sign-macos-binaries.sh <target-triple> <identity>
#   <identity> is "Developer ID Application: <name> (<team id>)" in CI, or "-"
#   to sign ad hoc when trying the script locally.
# Optional environment: CODESIGN_KEYCHAIN, the keychain that holds <identity>.
#
# Why deps/: on every build, even when nothing is recompiled, cargo copies each
# binary from target/<triple>/dist/deps/<crate>-<hash> to
# target/<triple>/dist/<bin>. Signing only target/<triple>/dist/<bin> is undone
# by the second `dist build`, so this signs the deps/ file that cargo copies
# from. scripts/release/verify-macos-archives.sh checks the archives afterwards,
# so if cargo ever stops doing this the release fails instead of shipping
# unsigned binaries.
set -euo pipefail

usage="usage: sign-macos-binaries.sh <target-triple> <identity>"
target="${1:?$usage}"
identity="${2:?$usage}"

metadata="$(cargo metadata --format-version 1 --no-deps)"
target_dir="$(jq -r .target_directory <<<"$metadata")"
workspace_root="$(jq -r .workspace_root <<<"$metadata")"
out_dir="$target_dir/$target/dist"

sign_args=(--force --options runtime --sign "$identity")
if [ "$identity" = "-" ]; then
  sign_args+=(--timestamp=none)
else
  # Developer ID signatures need a secure timestamp to be notarized.
  sign_args+=(--timestamp)
fi
if [ -n "${CODESIGN_KEYCHAIN:-}" ]; then
  sign_args+=(--keychain "$CODESIGN_KEYCHAIN")
fi

for bin in aranet aranet-gui; do
  built="$out_dir/$bin"
  if [ ! -f "$built" ]; then
    echo "error: $built does not exist; run dist build first" >&2
    exit 1
  fi

  # Without --identifier, codesign derives it from the hashed deps/ file name.
  # v0.2.0 shipped "aranet"; keep identifiers stable across releases.
  bin_args=(--identifier "$bin")
  if [ "$bin" = "aranet-gui" ]; then
    # The same entitlements as Aranet.app (.github/workflows/release-macos-app.yml).
    bin_args+=(--entitlements "$workspace_root/crates/aranet-gui/entitlements.plist")
  fi

  signed=0
  for candidate in "$out_dir"/deps/*; do
    if [ -f "$candidate" ] && [ -x "$candidate" ] && cmp -s "$candidate" "$built"; then
      echo "signing $candidate (cargo copies it to $built)"
      codesign "${sign_args[@]}" "${bin_args[@]}" "$candidate"
      codesign --verify --strict --verbose=2 "$candidate"
      signed=$((signed + 1))
    fi
  done
  if [ "$signed" -eq 0 ]; then
    echo "error: no copy of $built in $out_dir/deps; cargo's output layout has changed" >&2
    exit 1
  fi
done
