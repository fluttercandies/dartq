#!/usr/bin/env bash
# package.sh — stage one release tarball plus its sha256 sidecar.
#
# Usage: package.sh <binary-path> <ver> <os> <arch> <out-dir>
#
# Produces in <out-dir>:
#   dartq-<ver>-<os>-<arch>.tar.gz          the `dartq` binary + root LICENSE files
#   dartq-<ver>-<os>-<arch>.tar.gz.sha256   "<hex>  <filename>" (sha256sum -c form)
#
# The digest tool is whichever of sha256sum (Linux) or `shasum -a 256`
# (macOS) is present; the sidecar is written by hand so its format is
# identical on both (digest, two spaces, bare filename).
set -euo pipefail

usage() {
  echo "usage: $0 <binary-path> <ver> <os> <arch> <out-dir>" >&2
}

if [ "$#" -ne 5 ]; then
  usage
  exit 2
fi

binary="$1"
ver="$2"
os="$3"
arch="$4"
out_dir="$5"

if [ ! -f "$binary" ]; then
  echo "package.sh: binary not found: $binary" >&2
  exit 1
fi
for field in "$ver" "$os" "$arch" "$out_dir"; do
  if [ -z "$field" ]; then
    usage
    exit 2
  fi
done

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
name="dartq-$ver-$os-$arch"
tarball="$out_dir/$name.tar.gz"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

entries=(dartq)
install -m 0755 "$binary" "$stage/dartq"
for lic in LICENSE LICENSE-APACHE LICENSE-MIT; do
  if [ -f "$repo_root/$lic" ]; then
    cp "$repo_root/$lic" "$stage/$lic"
    entries+=("$lic")
  fi
done

mkdir -p "$out_dir"
tar -czf "$tarball" -C "$stage" "${entries[@]}"

if command -v sha256sum >/dev/null 2>&1; then
  hex="$(sha256sum "$tarball" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
  hex="$(shasum -a 256 "$tarball" | cut -d' ' -f1)"
else
  echo "package.sh: neither sha256sum nor shasum is installed" >&2
  exit 1
fi
printf '%s  %s\n' "$hex" "$name.tar.gz" > "$tarball.sha256"

echo "package.sh: wrote $tarball"
echo "package.sh: wrote $tarball.sha256"
