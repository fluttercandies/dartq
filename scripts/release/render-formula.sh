#!/usr/bin/env bash
# render-formula.sh — render the Homebrew formula from packaged release assets.
#
# Usage: render-formula.sh <ver> <assets-dir> <out-file>
#
# Reads the three per-target .sha256 sidecars produced by package.sh from
# <assets-dir> (dartq-<ver>-macos-arm64, -macos-x86_64, -linux-x86_64),
# validates each digest, and substitutes them plus the version into
# packaging/homebrew/dartq.rb.tmpl. The rendered file is the complete
# formula, ready to be PR'd into the tap repository by hand.
set -euo pipefail

usage() {
  echo "usage: $0 <ver> <assets-dir> <out-file>" >&2
}

if [ "$#" -ne 3 ]; then
  usage
  exit 2
fi

ver="$1"
assets_dir="$2"
out_file="$3"
if [ -z "$ver" ] || [ -z "$assets_dir" ] || [ -z "$out_file" ]; then
  usage
  exit 2
fi

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
template="$repo_root/packaging/homebrew/dartq.rb.tmpl"
if [ ! -f "$template" ]; then
  echo "render-formula.sh: template not found: $template" >&2
  exit 1
fi

digest() {
  sidecar="$assets_dir/dartq-$ver-$1.tar.gz.sha256"
  if [ ! -f "$sidecar" ]; then
    echo "render-formula.sh: missing asset checksum: $sidecar" >&2
    exit 1
  fi
  hex="$(cut -d' ' -f1 "$sidecar")"
  if ! printf '%s' "$hex" | grep -Eq '^[0-9a-f]{64}$'; then
    echo "render-formula.sh: malformed digest in $sidecar: $hex" >&2
    exit 1
  fi
  printf '%s' "$hex"
}

sha_macos_arm64="$(digest macos-arm64)"
sha_macos_x86_64="$(digest macos-x86_64)"
sha_linux_x86_64="$(digest linux-x86_64)"
sha_linux_arm64="$(digest linux-arm64)"

sed \
  -e "s#@VERSION@#$ver#g" \
  -e "s#@SHA_MACOS_ARM64@#$sha_macos_arm64#g" \
  -e "s#@SHA_MACOS_X86_64@#$sha_macos_x86_64#g" \
  -e "s#@SHA_LINUX_X86_64@#$sha_linux_x86_64#g" \
  -e "s#@SHA_LINUX_ARM64@#$sha_linux_arm64#g" \
  "$template" > "$out_file"

leftover="$(grep -n '@[A-Z_0-9]*@' "$out_file" || true)"
if [ -n "$leftover" ]; then
  echo "render-formula.sh: unreplaced template markers:" >&2
  printf '%s\n' "$leftover" >&2
  exit 1
fi

echo "render-formula.sh: rendered $out_file"
