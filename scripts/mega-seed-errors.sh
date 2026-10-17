#!/usr/bin/env bash
# mega-seed-errors.sh — deterministic seeded-error variant of a generated
# mega scale tree (the mega T2 oracle differential leg, the mega acceptance-bar
# ledger + the #733 vacuity ruling).
#
# Copies <src> (a corpus-gen tree, seed-0, package config materialized by
# mega-run.sh) to <dst> and injects the checked-in manifest's error files
# (scripts/mega-seeded-errors.json): each entry is one NEW standalone .dart
# file whose verbatim content carries exactly one deliberate ERROR-tier
# diagnostic from the frozen M1 gate set (tests/harness/m1_codes.txt —
# shapes dartq provably reports today). Injections are additive-only: a
# path that already exists in the source tree is refused, so generated
# content is never patched and the differential's (code, file) expectations
# stay auditable against the manifest alone.
#
# Determinism: no RNG, no time, no hashing — the same <src> tree plus the
# same manifest produce a byte-identical <dst> (<dst> is refreshed from
# scratch on every run). The manifest is the single source of truth for
# every injected (file, code, span).
#
# No analysis runs here: the script only copies files and writes the
# manifest's verbatim contents (no dart, no cargo). The seeded tree is
# analyzed in the serial verification phase through the harness
# (scripts/mega-run.sh --oracle-leg).
#
# Usage:
#   scripts/mega-seed-errors.sh [--src DIR] [--dst DIR] [--manifest FILE]
#                               [--dry-run] [--help]
#
#   --src DIR         generated source tree (default
#                     tests/corpus/mega/1k; must carry manifest.json).
#   --dst DIR         seeded variant tree (default
#                     tests/corpus/mega/1k-seeded; must stay under
#                     tests/corpus/mega/ — it is removed and rebuilt).
#   --manifest FILE   checked-in injection manifest (default
#                     scripts/mega-seeded-errors.json).
#   --dry-run         validate the manifest against the source tree and
#                     print the plan; execute nothing (no rm, no copy, no
#                     writes).
#
# Exit codes: 0 the seeded tree is ready (or the dry-run plan validated);
# 2 usage/environment error — missing jq, missing or malformed manifest,
# source tree not a generated corpus, injection path colliding with
# generated content, copy failure.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

SRC="tests/corpus/mega/1k"
DST="tests/corpus/mega/1k-seeded"
MANIFEST="scripts/mega-seeded-errors.json"
dry_run=0

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "mega-seed-errors: $*" >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
  --src)
    [ $# -ge 2 ] || die "--src needs a value"
    SRC="$2"
    shift
    ;;
  --dst)
    [ $# -ge 2 ] || die "--dst needs a value"
    DST="$2"
    shift
    ;;
  --manifest)
    [ $# -ge 2 ] || die "--manifest needs a value"
    MANIFEST="$2"
    shift
    ;;
  --dry-run) dry_run=1 ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/mega-seed-errors.sh --help)"
    ;;
  esac
  shift
done

# --- preflight + full manifest validation (both modes: a bad manifest must
# fail before any write) -------------------------------------------------------
command -v jq >/dev/null 2>&1 || die "jq is required to read the manifest (brew install jq)"
[ -f "$MANIFEST" ] || die "manifest $MANIFEST not found"
[ -f "$SRC/manifest.json" ] || die "$SRC is not a generated tree (no manifest.json) — regenerate it: cargo run -p corpus-gen -- --out $SRC --scale 1k --seed 0"
[ "$SRC" != "$DST" ] || die "--dst must differ from --src"
case "$DST" in
tests/corpus/mega/*) ;;
*) die "refusing to write outside tests/corpus/mega/ (got '$DST')" ;;
esac
jq -e '.injections | type == "array" and length > 0' "$MANIFEST" >/dev/null 2>&1 ||
  die "manifest $MANIFEST carries no non-empty injections array"
count="$(jq '.injections | length' "$MANIFEST")"

i=0
while [ "$i" -lt "$count" ]; do
  path="$(jq -r ".injections[$i].path" "$MANIFEST")"
  if [ -z "$path" ] || [ "$path" = "null" ]; then
    die "injections[$i]: missing path"
  fi
  case "$path" in
  /*) die "injections[$i]: path must be relative (got '$path')" ;;
  *..*) die "injections[$i]: path must not contain '..' (got '$path')" ;;
  *.dart) ;;
  *) die "injections[$i]: path must end in .dart (got '$path')" ;;
  esac
  if [ -e "$SRC/$path" ]; then
    die "injections[$i]: '$path' already exists in $SRC — injections are additive-only (generated content is never patched)"
  fi
  jq -e ".injections[$i].content | type == \"string\" and length > 0" "$MANIFEST" >/dev/null 2>&1 ||
    die "injections[$i] ($path): missing non-empty content"
  jq -e ".injections[$i].expect.code | type == \"string\" and length > 0" "$MANIFEST" >/dev/null 2>&1 ||
    die "injections[$i] ($path): missing expect.code"
  i=$((i + 1))
done

fp="$(jq -r '.fingerprint // "?"' "$SRC/manifest.json")"

if [ "$dry_run" -eq 1 ]; then
  echo "== mega-seed-errors plan"
  printf '  %-16s %s\n' "src" "$SRC"
  printf '  %-16s %s (refreshed from scratch)\n' "dst" "$DST"
  printf '  %-16s %s\n' "manifest" "$MANIFEST"
  printf '  %-16s %s\n' "src_fingerprint" "$fp"
  i=0
  while [ "$i" -lt "$count" ]; do
    path="$(jq -r ".injections[$i].path" "$MANIFEST")"
    code="$(jq -r ".injections[$i].expect.code" "$MANIFEST")"
    printf '  %-16s %s (expect: %s)\n' "inject[$i]" "$path" "$code"
    i=$((i + 1))
  done
  echo "== dry run: $count injection(s) validated, nothing written"
  exit 0
fi

# --- run: refresh dst from scratch, then write every injection verbatim ------
rm -rf "$DST" || die "cannot remove stale $DST"
cp -R "$SRC" "$DST" || die "copying $SRC -> $DST failed"
if [ ! -f "$DST/.dart_tool/package_config.json" ]; then
  echo "mega-seed-errors: WARN $SRC carries no .dart_tool/package_config.json (mega-run materializes it before this step; the harness corpus preflight refuses a tree without one)" >&2
fi

i=0
while [ "$i" -lt "$count" ]; do
  path="$(jq -r ".injections[$i].path" "$MANIFEST")"
  full="$DST/$path"
  mkdir -p "$(dirname "$full")" || die "creating the directory for $path failed"
  jq -j ".injections[$i].content" "$MANIFEST" >"$full" || die "writing $path failed"
  printf '  injected %s\n' "$path"
  i=$((i + 1))
done

echo "mega-seed-errors: $count file(s) injected into $DST (base fingerprint $fp; the manifest is the injection identity)"
