#!/usr/bin/env bash
# sem2-gate.sh — the semantic wave-2 differential gate (the nailed-but-
# ungated divergence surface, the natural continuation of the m1/m2 tiers).
#
# Runs the harness once over tests/corpus/sem2 with the frozen semantic
# wave-2 code universe (tests/harness/sem2_codes.txt), ERROR severity,
# threshold 1.0 — the 100% release caliber (ticket #1293), same gate shape as
# every differential gate: it passes only when the ERROR-tier diff plane is
# empty after the known-diffs exemption (tests/corpus/regressions/
# known-diffs.json — matched entries are EXEMPTED, anything else is
# unattributed divergence and fails; the raw pre-exemption Jaccard is a
# TELEMETRY row). Oracle of record: the fvm default install
# (Dart 3.13.1). The tier's derivation pin
# (sem2_codes_file_matches_derivation, tests/harness/src/main.rs) keeps the
# codes file equal to its live derivation from the regression nails, so the
# universe grows automatically in review terms: a fresh nail code fails the
# pin and forces a conscious re-freeze before it can be gated.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over the corpus. Never run it alongside development-phase lanes
# (memory-incident discipline). --dry-run executes nothing.
#
# Usage:
#   scripts/sem2-gate.sh [--dry-run] [--label L] [--allow-vacuous]
#   scripts/sem2-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact harness
#                   command, execute nothing (no dart, no harness).
#   --label L       report directory name under target/evidence/gate-sem2/.
#                   Default: SEM2_LABEL env, else sem2-r1. An existing report
#                   directory is refused — a rerun never overwrites recorded
#                   evidence.
#   --allow-vacuous forwarded to the harness: accept a run whose oracle side
#                   has no ERROR-tier entries post-filter. Without it such a
#                   run is a harness error — the gate would pass at 1.000
#                   without judging anything (a broken codes file, a wrong
#                   corpus, an oracle that silently produced nothing).
#   Every run persists post-filter divergences as regression cases under
#   tests/corpus/regressions/cases (exempted against
#   tests/corpus/regressions/known-diffs.json).
#
# Environment overrides: FVM_HOME (default ~/fvm; the default install must
# exist — its Dart version banner is checked on the run path and a mismatch
# is FATAL; DARTQ_ORACLE_VERSION re-pins the expectation,
# DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate off-caliber oracle as
# a warning), DARTQ_CMD (the dartq side of the harness; default
# `./target/debug/dartq analyze --format=json --sdk <fvm default
# dart-sdk>`), DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded to the harness
# drivers).
#
# Exit codes: 0 gate met (ERROR-tier diff plane empty after the known-diffs
# exemption — set equality, ticket #1293); 1 unattributed divergence;
# 2 usage or environment error — missing binaries, fvm default missing, jq
# missing, unreadable report, malformed known-diffs table — and the harness's
# own error tier (exit 2: violated vacuity guard, dartq crash, corpus
# preflight failure), which is never scored as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2
. "$(dirname "$0")/lib/diff-score.sh"

CORPUS="tests/corpus/sem2"
CODES_FILE="tests/harness/sem2_codes.txt"
THRESHOLD="1.0"
KNOWN_DIFFS="tests/corpus/regressions/known-diffs.json"
EXPECTED_VERSION="${DARTQ_ORACLE_VERSION:-3.13.1}"
OUT_ROOT="target/evidence/gate-sem2"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="sem2-r1"
EXPECTED_CODES=77

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_vacuous=0
label="${SEM2_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "sem2-gate: --label needs a value" >&2
      exit 2
    }
    label="$2"
    shift
    ;;
  --allow-vacuous) allow_vacuous=1 ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "sem2-gate: unknown argument '$1' (see scripts/sem2-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$CODES_FILE" ]; then
  echo "sem2-gate: $CODES_FILE not found (the frozen semantic wave-2 code universe)" >&2
  exit 2
fi
# Frozen-universe count pin: this file is the gate's denominator, and a
# hand-truncation during a window would otherwise pass with no runtime
# trace. Rows are counted exactly as the harness's parse_codes_file reads
# them (trim; skip blank and '#'-comment lines): the frozen wave-2 universe
# is 77 codes at the 2026-10-18 re-freeze (waves 312ed6c/3b239d6 added 12
# rows over the 2026-10-13 freeze; the derivation pin was green at HEAD
# before this bump). The derivation pin
# (sem2_codes_file_matches_derivation) re-derives the set live in `cargo
# test -p harness`; this count is the cheap runtime cross-check.
sem2_count="$(grep -cv -e '^[[:space:]]*#' -e '^[[:space:]]*$' "$CODES_FILE")"
[ "$sem2_count" -eq "$EXPECTED_CODES" ] || {
  echo "sem2-gate: $CODES_FILE has $sem2_count data rows, expected $EXPECTED_CODES — the frozen wave-2 universe was truncated or drifted" >&2
  exit 2
}
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "sem2-gate: $CORPUS is not a corpus package" >&2
  exit 2
fi
if [ ! -d "$CORPUS/.dart_tool" ]; then
  echo "sem2-gate: $CORPUS has no .dart_tool — pub get the corpus first" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "sem2-gate: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "sem2-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
if [ ! -x "$dart_bin" ]; then
  echo "sem2-gate: fvm default dart not found at $dart_bin" >&2
  echo "  install the fvm default pin (dart 3.13.1) or point FVM_HOME at it;" >&2
  echo "  to use another oracle run the harness directly with --oracle-dart." >&2
  exit 2
fi

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=json --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "sem2-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
if [ -e "$outdir" ]; then
  echo "sem2-gate: $outdir already exists — refusing to overwrite recorded gate evidence (pass a new --label)" >&2
  exit 2
fi
echo "== sem2-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s %s (ERROR tier, raw threshold %s — telemetry; the gate is the 100% caliber)\n' "codes" "$CODES_FILE" "$THRESHOLD"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions tests/corpus/regressions/cases\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$CODES_FILE" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version gate (run path only; the report meta records the real
# banner either way). Sem-wave-2 semantics are pinned to the 3.13.1 caliber
# — a different oracle can legitimately emit a different code set, so a
# mismatch is FATAL. Escapes: DARTQ_ORACLE_VERSION re-pins the expectation,
# DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate off-caliber oracle as
# a warning record.
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  if [ "${DARTQ_ORACLE_VERSION_CHECK:-}" = "off" ]; then
    echo "sem2-gate: WARN dart $EXPECTED_VERSION mismatch accepted (DARTQ_ORACLE_VERSION_CHECK=off), found '${found:-unrecognized banner}'"
  else
    echo "sem2-gate: FATAL expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}' — gate semantics are version-pinned; install the fvm default pin or point FVM_HOME at it, set DARTQ_ORACLE_VERSION to the oracle's version, or set DARTQ_ORACLE_VERSION_CHECK=off to accept a deliberate off-caliber oracle" >&2
    exit 2
  fi
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes: 0 the RAW (pre-exemption) ERROR Jaccard met the
# threshold, 1 below it (the report is still written either way), 2 harness
# error (vacuity guard, dartq crash, corpus preflight — no verdict scored).
# The harness verdict is exemption-blind, so rc 1 is NOT the gate verdict:
# the 100% scoring below classifies every diff entry against the known-diffs
# ledger and fails only on unattributed divergence.
run_args=(
  run
  --corpus-dir "$CORPUS"
  --oracle-dart "$dart_bin"
  --dartq-cmd "$DARTQ_CMD"
  --severity-filter ERROR
  --threshold "$THRESHOLD"
  --codes-file "$CODES_FILE"
  # Divergences land in tests/corpus/regressions/cases, exempted via known-diffs.json.
  --persist-regressions tests/corpus/regressions/cases
  --out "$outdir"
)
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] sem2 gate"
mkdir -p "$outdir"
rc=0
"$HARNESS" "${run_args[@]}" || rc=$?
if [ "$rc" -ge 2 ]; then
  echo "-- [$label] gate ERRORED (harness exit $rc, harness-own failure; no verdict scored)" >&2
  exit 2
fi
if [ "$rc" -eq 0 ]; then
  echo "-- [$label] raw bar met (zero diffs); report: $outdir/report.json"
else
  echo "-- [$label] below the raw bar (harness exit 1); scoring the known-diffs exemption from the report"
fi

# --- summary -----------------------------------------------------------------
row="$(jq -r '
  [ (.error_jaccard * 1000 | round / 1000 | tostring),
    (.oracle_total | tostring),
    (.dartq_total | tostring),
    (.oracle_only | length | tostring),
    (.dartq_only | length | tostring)
  ] | @tsv' "$outdir/report.json" 2>/dev/null)"
if [ -z "$row" ]; then
  echo "sem2-gate: report $outdir/report.json is missing or unreadable" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF

# 100% scoring (ticket #1293): classify every flat diff entry against the
# known-diffs ledger. Empty output = unreadable report or malformed table.
classification="$(diff_gate_classify "$outdir/report.json" "$KNOWN_DIFFS")"
if [ -z "$classification" ]; then
  echo "sem2-gate: cannot classify $outdir/report.json against $KNOWN_DIFFS (report unreadable or the known-diffs table malformed)" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r _ exempted unattributed <<EOF
$(printf '%s\n' "$classification" | awk -F'\t' '$1 == "SUMMARY" {print; exit}')
EOF
echo
echo "== sem2-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (TELEMETRY: raw, pre-exemption — never a gate)\n' "error_jaccard" "$jac"
printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
printf '  %-16s %s matched by the known-diffs ledger (the burn-down mechanism)\n' "exempted" "$exempted"
printf '  %-16s %s (gate caliber: 0 — any unattributed divergence fails)\n' "unattributed" "$unattributed"
if [ "$unattributed" -gt 0 ]; then
  printf '%s\n' "$classification" | awk -F'\t' '$1 == "UNATTRIBUTED" { printf "    %-12s %s %s\n", $2, $3, $4 }'
fi
printf '  %-16s %s\n' "report" "$outdir/report.json"
if [ "$unattributed" -eq 0 ]; then
  echo "sem2-gate: VERDICT PASS (100% caliber: every ERROR-tier diff entry is matched or exempted)"
  exit 0
fi
echo "sem2-gate: VERDICT FAIL ($unattributed unattributed divergence entries — fix, or attribute in $KNOWN_DIFFS with a reason; the gate never passes on a numeric bar)"
exit 1
