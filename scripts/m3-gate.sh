#!/usr/bin/env bash
# m3-gate.sh — the M3 unused-family differential gate (goal #764).
#
# One harness run over tests/corpus/m3-unused with the frozen M3 code set
# (tests/harness/m3_unused_codes.txt), WARNING severity, bar 0.95, against
# the same single oracle of record as M1/M2: the fvm default install. All
# ten codes are registry STATIC_WARNING (see the codes-file header), so the
# WARNING tier carries the whole family and the codes file scopes the run.
#
# The harness's PASS/FAIL verdict is hard-keyed to the ERROR tier, so a
# WARNING-only run reports verdict SKIPPED and exits 0 by design; this
# script therefore scores the WARNING tier from the report's stats and
# enforces the WARNING-tier vacuity guard itself — same as m2-gate.sh.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over the corpus. Never run it alongside development-phase lanes
# (memory-incident discipline). --dry-run resolves every path and executes
# nothing.
#
# Usage:
#   scripts/m3-gate.sh [--dry-run]
#
# Environment overrides: FVM_HOME (default ~/fvm), DARTQ_CMD (the dartq
# side of the harness), M3_LABEL (report directory name under
# docs/audits/gate-m3/, default m3-r1; an existing report directory is
# refused — a rerun never overwrites recorded evidence).
#
# Exit codes: 0 gate met (WARNING-tier Jaccard >= 0.95); 1 below the bar;
# 2 usage or environment error, or the harness's own error tier (dartq
# crash, corpus preflight failure) — never scored as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/m3-unused"
CODES_FILE="tests/harness/m3_unused_codes.txt"
THRESHOLD="0.95"
OUT_ROOT="docs/audits/gate-m3"
HARNESS="./target/debug/harness"
label="${M3_LABEL:-m3-r1}"

dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  *) echo "m3-gate: unknown argument '$1' (usage: scripts/m3-gate.sh [--dry-run])" >&2; exit 2 ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
for path in "$CODES_FILE" "$CORPUS/pubspec.yaml" "$CORPUS/.dart_tool" "$HARNESS"; do
  [ -e "$path" ] || { echo "m3-gate: $path not found (build the harness: cargo build -p harness; pub get the corpus: cd $CORPUS && dart pub get)" >&2; exit 2; }
done
command -v jq >/dev/null 2>&1 || { echo "m3-gate: jq is required for the scoring (brew install jq)" >&2; exit 2; }
sdk_root="${FVM_HOME:-$HOME/fvm}/default/bin/cache/dart-sdk"
[ -x "$sdk_root/bin/dart" ] || { echo "m3-gate: fvm default dart not found at $sdk_root/bin/dart (install the fvm default pin or set FVM_HOME)" >&2; exit 2; }
DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "m3-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
if [ -e "$outdir" ]; then
  echo "m3-gate: $outdir already exists — refusing to overwrite recorded gate evidence (set a new M3_LABEL)" >&2
  exit 2
fi
echo "== m3-gate [$label]: corpus=$CORPUS codes=$CODES_FILE (WARNING tier, bar $THRESHOLD)"
echo "== oracle=$sdk_root/bin/dart dartq=$DARTQ_CMD report=$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes on a WARNING-only run: 0 (verdict SKIPPED — the harness
# cannot judge this tier) or 2 harness-own failure. A judged exit 1 is
# structurally impossible here; the verdict comes from the scoring below.
mkdir -p "$outdir"
if ! "$HARNESS" run --corpus-dir "$CORPUS" --oracle-dart "$sdk_root/bin/dart" --dartq-cmd "$DARTQ_CMD" \
  --severity-filter WARNING --threshold "$THRESHOLD" --codes-file "$CODES_FILE" --out "$outdir"; then
  echo "m3-gate: gate ERRORED (harness-own failure; no verdict scored)" >&2
  exit 2
fi

# --- score the WARNING tier ---------------------------------------------------
row="$(jq -r '.severities[] | select(.severity == "WARNING") | [(.intersection|tostring), (.oracle_only|tostring), (.dartq_only|tostring)] | @tsv' "$outdir/report.json" 2>/dev/null)"
[ -n "$row" ] || { echo "m3-gate: report $outdir/report.json is missing or unreadable, or carries no WARNING tier" >&2; exit 2; }
IFS="$(printf '\t')" read -r inter oonly donly <<EOF
$row
EOF
# Vacuity guard (the harness's own is ERROR-keyed): zero oracle-side entries
# post-filter means the gate scored without judging anything (a broken codes
# file, a wrong corpus, or an oracle that silently produced nothing).
[ "$((inter + oonly))" -gt 0 ] || { echo "m3-gate: ERRORED — no oracle-side WARNING entries post-filter; the gate would pass without judging anything" >&2; exit 2; }
echo "== m3-gate [$label]: intersection=$inter oracle_only=$oonly dartq_only=$donly report=$outdir/report.json"
# Exact decision on the raw counts: i/u >= 0.95 with u = intersection +
# oracle-only + dartq-only (the Jaccard union) is 20*i >= 19*u — integer,
# no rounding.
union=$((inter + oonly + donly))
if [ "$((20 * inter))" -ge "$((19 * union))" ]; then
  echo "m3-gate: VERDICT PASS (WARNING-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "m3-gate: VERDICT FAIL (below the $THRESHOLD bar)"
exit 1
