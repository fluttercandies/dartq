#!/usr/bin/env bash
# sem2-gate.sh — the semantic wave-2 differential gate (the nailed-but-
# ungated divergence surface, the natural continuation of the m1/m2 tiers).
#
# Runs the harness once over tests/corpus/sem2 with the frozen semantic
# wave-2 code universe (tests/harness/sem2_codes.txt), ERROR severity,
# threshold 1.0 — the full-parity bar, same caliber as every differential
# gate — against the single oracle of record: the fvm default install
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
# exist — its Dart version banner is checked on the run path, a mismatch is
# a WARN, not fatal), DARTQ_CMD (the dartq side of the harness; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`), DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded to the harness
# drivers).
#
# Exit codes: 0 gate met (ERROR-tier Jaccard >= 1.0); 1 below the bar;
# 2 usage or environment error — missing binaries, fvm default missing, jq
# missing, unreadable report — and the harness's own error tier (exit 2:
# violated vacuity guard, dartq crash, corpus preflight failure), which is
# never scored as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/sem2"
CODES_FILE="tests/harness/sem2_codes.txt"
THRESHOLD="1.0"
EXPECTED_VERSION="3.13.1"
OUT_ROOT="target/evidence/gate-sem2"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="sem2-r1"
EXPECTED_CODES=65

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
# is 65 codes at the 2026-10-13 derivation. The derivation pin
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

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
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
printf '  %-16s %s (ERROR tier, bar %s)\n' "codes" "$CODES_FILE" "$THRESHOLD"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions tests/corpus/regressions/cases\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$CODES_FILE" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version banner (run path only; the report meta records the real
# banner either way).
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  echo "sem2-gate: WARN expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}'"
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes: 0 PASS, 1 below threshold (the report is still
# written), 2 harness error (vacuity guard, dartq crash, corpus preflight —
# no verdict is scored).
run_args=(
  run
  --corpus-dir "$CORPUS"
  --oracle-dart "$dart_bin"
  --dartq-cmd "$DARTQ_CMD"
  --severity-filter ERROR
  --threshold "$THRESHOLD"
  --codes-file "$CODES_FILE"
  # Divergences land in tests/corpus/regressions/cases, exempted via tests/corpus/regressions/known-diffs.json.
  --persist-regressions tests/corpus/regressions/cases
  --out "$outdir"
)
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] sem2 gate"
mkdir -p "$outdir"
verdict=PASS
if "$HARNESS" "${run_args[@]}"; then
  echo "-- [$label] gate PASSED the bar; report: $outdir/report.json"
else
  rc=$?
  if [ "$rc" -eq 1 ]; then
    verdict=FAIL
    echo "-- [$label] gate FAILED below the bar (harness exit 1); report: $outdir/report.json"
  else
    echo "-- [$label] gate ERRORED (harness exit $rc, harness-own failure; no verdict scored)" >&2
    exit 2
  fi
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
echo
echo "== sem2-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (bar %s)\n' "error_jaccard" "$jac" "$THRESHOLD"
printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
printf '  %-16s %s\n' "report" "$outdir/report.json"
if [ "$verdict" = "PASS" ]; then
  echo "sem2-gate: VERDICT PASS (ERROR-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "sem2-gate: VERDICT FAIL (below the $THRESHOLD bar)"
exit 1
