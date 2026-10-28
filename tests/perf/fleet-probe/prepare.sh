#!/usr/bin/env bash
# prepare.sh — build the fleet layout for the T1 multi-instance RSS probe
# (ADR-multi-worktree-posture ticket T1).
#
# The repo's discipline bans creating git worktrees for any purpose, so the
# probe simulates the agent fleet with PLAIN DIRECTORY COPIES of a fixture
# corpus into N sibling directories. The sharing axis the ADR models is
# CONTENT IDENTITY (ContentId-keyed machine-global L2/L3 stores), not git
# mechanics: byte-identical copies make every instance produce the same
# content keys, which is exactly the worktree fleet's shape. Copies are
# self-contained packages — the mega corpora's pubspec.yaml and
# .dart_tool/package_config.json use relative paths only — so each daemon
# pointed at a copy analyzes its own copy's bytes.
#
# Fixture source: DARTQ_PROBE_SOURCE must name a package directory (the
# repo no longer vendors a corpus; point it at a pub-cache package checkout
# or any generated tree — byte-identical copies of it feed the N instances).
#
# Usage: prepare.sh [--dry-run] [--clean] <scale> <N>
#   scale in {1k,10k,100k}; N >= 1 copies.
# Copies land in "$DARTQ_FLEET_ROOT/<scale>/copy-001..N" (default root:
# "${TMPDIR:-/tmp}/dartq-fleet"). Idempotent: existing non-empty copies are
# skipped unless --clean or DARTQ_PROBE_FORCE=1.
#
# Disk headroom: refuses to copy unless the destination volume holds
# N x source-size x 1.1 + 1 GiB margin, and prints the largest N that
# would fit when refusing — use that arithmetic to descope N rather than
# failing mid-fleet. (APFS may store the copies as clones and cost far
# less; the check stays conservative on purpose.)
set -euo pipefail

PROBE_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$PROBE_DIR/../../.." && pwd)"

DRY_RUN=0
CLEAN=0
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --clean) CLEAN=1 ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) ARGS+=("$arg") ;;
  esac
done
if [ "${#ARGS[@]}" -ne 2 ]; then
  echo "usage: prepare.sh [--dry-run] [--clean] <scale:1k|10k|100k> <N>=1" >&2
  exit 2
fi

SCALE="${ARGS[0]}"
N="${ARGS[1]}"
case "$SCALE" in
  1k|10k|100k) ;;
  *) echo "prepare.sh: scale must be one of 1k, 10k, 100k, got '$SCALE'" >&2; exit 2 ;;
esac
case "$N" in
  ''|*[!0-9]*) echo "prepare.sh: N must be a non-negative integer, got '$N'" >&2; exit 2 ;;
esac
if [ "$N" -lt 1 ]; then
  echo "prepare.sh: N must be >= 1, got '$N'" >&2
  exit 2
fi

SOURCE="${DARTQ_PROBE_SOURCE:-}"
FLEET_ROOT="${DARTQ_FLEET_ROOT:-${TMPDIR:-/tmp}/dartq-fleet}"
DEST_BASE="$FLEET_ROOT/$SCALE"

if [ -z "$SOURCE" ]; then
  echo "prepare.sh: DARTQ_PROBE_SOURCE is required — set it to a package fixture" \
       "directory (pubspec.yaml + .dart_tool/package_config.json)" >&2
  exit 2
fi

if [ ! -f "$SOURCE/pubspec.yaml" ] || [ ! -f "$SOURCE/.dart_tool/package_config.json" ]; then
  echo "prepare.sh: '$SOURCE' is not a package fixture (need pubspec.yaml and" \
       ".dart_tool/package_config.json)" >&2
  exit 2
fi

SOURCE_KIB="$(du -sk "$SOURCE" | awk '{print $1}')"
MARGIN_KIB=1048576   # 1 GiB
REQUIRED_KIB=$(( SOURCE_KIB * N * 11 / 10 + MARGIN_KIB ))

mkdir -p "$FLEET_ROOT"
AVAIL_KIB="$(df -k "$FLEET_ROOT" | awk 'NR==2 {print $4}')"
MAX_FIT_N=$(( (AVAIL_KIB - MARGIN_KIB) / (SOURCE_KIB * 11 / 10) ))

echo "prepare.sh: scale=$SCALE N=$N source=$SOURCE (${SOURCE_KIB} KiB)"
echo "prepare.sh: dest=$DEST_BASE free=${AVAIL_KIB} KiB required=${REQUIRED_KIB} KiB (N x source x 1.1 + 1 GiB margin)"

if [ "$AVAIL_KIB" -lt "$REQUIRED_KIB" ]; then
  echo "prepare.sh: INSUFFICIENT DISK — need ${REQUIRED_KIB} KiB, have ${AVAIL_KIB} KiB." >&2
  echo "prepare.sh: largest N that fits this volume: $MAX_FIT_N. Descope N to that (the phi" >&2
  echo "  formula in analyze.py works for any N >= 2) or point DARTQ_FLEET_ROOT at a" >&2
  echo "  larger volume and re-run." >&2
  exit 2
fi

if [ "$DRY_RUN" -eq 1 ]; then
  echo "prepare.sh: dry-run — would copy $N directories:"
  i=1
  while [ "$i" -le "$N" ]; do
    printf '  %s/copy-%03d\n' "$DEST_BASE" "$i"
    i=$(( i + 1 ))
  done
  exit 0
fi

if [ "$CLEAN" -eq 1 ]; then
  echo "prepare.sh: --clean removing $DEST_BASE"
  rm -rf "$DEST_BASE"
fi

i=1
while [ "$i" -le "$N" ]; do
  DEST="$DEST_BASE/copy-$(printf '%03d' "$i")"
  if [ -d "$DEST" ] && [ -n "$(ls -A "$DEST" 2>/dev/null)" ] && [ "${DARTQ_PROBE_FORCE:-0}" != "1" ]; then
    echo "prepare.sh: skip existing $DEST (DARTQ_PROBE_FORCE=1 to recopy)"
  else
    mkdir -p "$DEST"
    cp -R "$SOURCE/." "$DEST/"
    echo "prepare.sh: copied $DEST"
  fi
  i=$(( i + 1 ))
done

echo "prepare.sh: done — fleet at $DEST_BASE ($(du -sk "$DEST_BASE" | awk '{print $1}') KiB on disk)"
