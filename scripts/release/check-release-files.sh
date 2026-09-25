#!/usr/bin/env bash
# Check a directory holding a release's files, either this run's artifacts before
# publishing (release-smoke-test.yml) or a published GitHub Release
# (release-verify.yml):
#   - sha256.sum and every <file>.sha256 match the files (`sha256sum -c`),
#   - every archive has a <archive>.sha256, and sha256.sum lists the archive
#     with that checksum,
#   - aranet-cli-installer.sh and aranet-gui-installer.sh offer every archive of
#     their app, with the archive's current checksum,
#   - the .ps1 installers offer every Windows archive (dist 0.31's PowerShell
#     installer embeds no checksums).
#
# Usage: scripts/release/check-release-files.sh <dir>
set -euo pipefail
shopt -s nullglob

cd "${1:?usage: check-release-files.sh <dir>}"

if command -v sha256sum > /dev/null; then
  check() { sha256sum -c "$1"; }
else
  check() { shasum -a 256 -c "$1"; }
fi
check sha256.sum
for f in *.sha256; do
  check "$f"
done

for app in aranet-cli aranet-gui; do
  archives=("$app"-*.tar.xz "$app"-*.zip)
  if [ "${#archives[@]}" -eq 0 ]; then
    echo "error: no $app archives in $PWD" >&2
    exit 1
  fi
  for archive in "${archives[@]}"; do
    if [ ! -f "$archive.sha256" ]; then
      echo "error: $archive has no $archive.sha256" >&2
      exit 1
    fi
    sum="$(cut -d' ' -f1 "$archive.sha256")"
    if ! grep -qxF "$sum *$archive" sha256.sum; then
      echo "error: sha256.sum does not list $archive" >&2
      exit 1
    fi
    if ! grep -qF "\"$archive\")" "$app-installer.sh"; then
      echo "error: $app-installer.sh does not offer $archive" >&2
      exit 1
    fi
    if ! grep -qF "_checksum_value=\"$sum\"" "$app-installer.sh"; then
      echo "error: $app-installer.sh has a stale checksum for $archive" >&2
      exit 1
    fi
    if [[ "$archive" == *.zip ]] && ! grep -qF "\"artifact_name\" = \"$archive\"" "$app-installer.ps1"; then
      echo "error: $app-installer.ps1 does not offer $archive" >&2
      exit 1
    fi
  done
  echo "ok: $app: ${#archives[@]} archives, listed in sha256.sum and both installers"
done
