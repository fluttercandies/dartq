#!/usr/bin/env bash
# m3-gate.sh — the M3 unused-family differential gate (goal #764).
#
# One harness run over tests/corpus/m3-unused with the frozen M3 code set
# (tests/harness/m3_unused_codes.txt), WARNING severity, threshold 1.0 — the
# 100% release caliber (ticket #1293; 2026-10-12 ruling, the full-parity bars
# ADR; supersedes the earlier numeric bars): the gate passes only when the
# WARNING-tier diff plane is empty after the known-diffs exemption — every
# entry is either matched by tests/corpus/regressions/known-diffs.json (the
# adjudicated burn-down ledger, reported as EXEMPTED) or it is unattributed
# divergence and fails the gate. There is no numeric pass bar; the raw
# (pre-exemption) Jaccard is a TELEMETRY row. Oracle of record: the same
# single oracle as M1/M2: the fvm default install. All
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
# Environment overrides: FVM_HOME (default ~/fvm; the default install's Dart
# version banner is checked on the run path and a mismatch is FATAL;
# DARTQ_ORACLE_VERSION re-pins the expectation, DARTQ_ORACLE_VERSION_CHECK=off
# accepts a deliberate off-caliber oracle as a warning), DARTQ_CMD (the dartq
# side of the harness), M3_LABEL (report
# directory name under target/evidence/gate-m3/, default m3-r1; an existing
# report directory is refused — a rerun never overwrites recorded evidence).
#
# Exit codes: 0 gate met (WARNING-tier diff plane empty after the known-diffs
# exemption — set equality, ticket #1293); 1 unattributed divergence; 2 usage
# or environment error, malformed known-diffs table, or the harness's own
# error tier (dartq crash, corpus preflight failure) — never scored as a
# Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2
. "$(dirname "$0")/lib/diff-score.sh"

CORPUS="tests/corpus/m3-unused"
CODES_FILE="tests/harness/m3_unused_codes.txt"
THRESHOLD="1.0"
KNOWN_DIFFS="tests/corpus/regressions/known-diffs.json"
EXPECTED_VERSION="${DARTQ_ORACLE_VERSION:-3.13.1}"
OUT_ROOT="target/evidence/gate-m3"
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
# Frozen-family count pin (#937): the freeze file is the gate's denominator,
# and a hand-truncation during a window would otherwise pass with no runtime
# trace. The expected size is DERIVED, not hardcoded: m3_codes_pin.rs's
# USAGE_FAMILY is the compile-checked registry derivation (it already names
# unused_field_from_primary_constructor, ahead of the re-freeze regen), so
# counting its members keeps this pin exact across the regen. Freeze rows are
# counted as the harness's parse_codes_file reads them (skip blank/'#' lines).
# The freeze has been regenerated to the full 11-code family (the pin is
# GREEN: freeze rows == family members, no regen pending) — the pin stays so
# the next registry-side family change trips here before the gate runs.
PIN_SRC="crates/dartq-semantic/tests/m3_codes_pin.rs"
[ -f "$PIN_SRC" ] || { echo "m3-gate: $PIN_SRC not found (the count pin's registry-derivation source)" >&2; exit 2; }
m3_expected="$(sed -n '/^const USAGE_FAMILY/,/^];/p' "$PIN_SRC" | grep -c 'SemanticCode::')"
[ -n "$m3_expected" ] && [ "$m3_expected" -gt 0 ] || {
  echo "m3-gate: cannot derive the expected freeze size from $PIN_SRC (USAGE_FAMILY moved?) — fix the pin's extraction" >&2
  exit 2
}
m3_count="$(grep -cv -e '^[[:space:]]*#' -e '^[[:space:]]*$' "$CODES_FILE")"
[ "$m3_count" -eq "$m3_expected" ] || {
  echo "m3-gate: $CODES_FILE has $m3_count data rows, registry-derived expectation $m3_expected — stale or drifted freeze; re-freeze with: cargo test -p dartq-semantic --test m3_codes_pin regen_m3_unused_codes -- --ignored (then reconcile the header prose)" >&2
  exit 2
}
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
echo "== m3-gate [$label]: corpus=$CORPUS codes=$CODES_FILE (WARNING tier, raw threshold $THRESHOLD — telemetry; the gate is the 100% caliber)"
echo "== oracle=$sdk_root/bin/dart dartq=$DARTQ_CMD report=$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version gate (run path only; the report meta records the real
# banner either way). M3 semantics are pinned to the 3.13.1 caliber — a
# different oracle can legitimately emit a different code set, so a
# mismatch is FATAL. Escapes: DARTQ_ORACLE_VERSION re-pins the expectation,
# DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate off-caliber oracle as
# a warning record.
found="$("$sdk_root/bin/dart" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  if [ "${DARTQ_ORACLE_VERSION_CHECK:-}" = "off" ]; then
    echo "m3-gate: WARN dart $EXPECTED_VERSION mismatch accepted (DARTQ_ORACLE_VERSION_CHECK=off), found '${found:-unrecognized banner}'"
  else
    echo "m3-gate: FATAL expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}' — gate semantics are version-pinned; install the fvm default pin or point FVM_HOME at it, set DARTQ_ORACLE_VERSION to the oracle's version, or set DARTQ_ORACLE_VERSION_CHECK=off to accept a deliberate off-caliber oracle" >&2
    exit 2
  fi
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
echo "== m3-gate [$label]: intersection=$inter oracle_only=$oonly dartq_only=$donly (raw, TELEMETRY — never a gate) report=$outdir/report.json"
# 100% scoring (ticket #1293): classify every flat diff entry against the
# known-diffs ledger. Empty output = unreadable report or malformed table.
classification="$(diff_gate_classify "$outdir/report.json" "$KNOWN_DIFFS")"
if [ -z "$classification" ]; then
  echo "m3-gate: cannot classify $outdir/report.json against $KNOWN_DIFFS (report unreadable or the known-diffs table malformed)" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r _ exempted unattributed <<EOF
$(printf '%s\n' "$classification" | awk -F'\t' '$1 == "SUMMARY" {print; exit}')
EOF
echo "== m3-gate [$label]: exempted=$exempted (known-diffs ledger) unattributed=$unattributed (gate caliber: 0)"
if [ "$unattributed" -gt 0 ]; then
  printf '%s\n' "$classification" | awk -F'\t' '$1 == "UNATTRIBUTED" { printf "    %-12s %s %s\n", $2, $3, $4 }'
fi
if [ "$unattributed" -eq 0 ]; then
  echo "m3-gate: VERDICT PASS (100% caliber: every WARNING-tier diff entry is matched or exempted)"
  exit 0
fi
echo "m3-gate: VERDICT FAIL ($unattributed unattributed divergence entries — fix, or attribute in $KNOWN_DIFFS with a reason; the gate never passes on a numeric bar)"
exit 1
