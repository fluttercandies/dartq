#!/usr/bin/env bash
# m2-gate.sh — the M2 WARNING-tier differential gate (PRD section 20.3, M2 row).
#
# Runs the harness once over tests/corpus/m2-warning with the frozen M2 code
# universe (tests/harness/m2_warning_codes.txt), WARNING severity, threshold
# 1.0 — the M2 bar, full parity like every differential gate (2026-10-12
# ruling, docs/facts/adr-2026-10-12-full-parity-bars.md; supersedes the PRD
# section 20.3 numeric bars; residuals pass only via individually adjudicated
# pinned exemptions, never a numeric budget) — against the same single oracle
# of record as M1: the fvm default install (Dart 3.13.1, the gate-caliber-r5
# baseline). The WARNING
# tier scores only semantic STATIC_WARNING codes; lint rules are INFO and
# excluded by design (the M2 audit's tier split). The 43-code frozen set's
# derivation lives in the codes-file header.
#
# The harness's PASS/FAIL verdict, exit-code gate metric, and non-vacuity
# guard are hard-keyed to the ERROR tier, so a WARNING-only run reports
# verdict SKIPPED and exits 0 by design. This script therefore scores the
# gate itself from the report's WARNING-tier stats and runs the WARNING-tier
# vacuity guard here — after the run, so unlike M1 a vacuous run has already
# materialized the report directory when the guard fires.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over the corpus. Never run it alongside development-phase lanes
# (memory-incident discipline). --dry-run executes nothing.
#
# Usage:
#   scripts/m2-gate.sh [--dry-run] [--label L] [--allow-vacuous]
#   scripts/m2-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact harness
#                   command, execute nothing (no dart, no harness).
#   --label L       report directory name under docs/audits/gate-m2/.
#                   Default: M2_LABEL env, else m2-r1. An existing report
#                   directory is refused — a rerun never overwrites recorded
#                   evidence.
#   --allow-vacuous forwarded to the harness and honored here: accept a run
#                   whose oracle side has no WARNING-tier entries post-filter.
#                   Without it such a run is a gate error — the WARNING tier
#                   would pass at 1.000 without judging anything (a broken
#                   codes file, a wrong corpus, an oracle that silently
#                   produced nothing).
#
# Environment overrides: FVM_HOME (default ~/fvm; the default install must
# exist — its Dart version banner is checked on the run path, a mismatch is
# a WARN, not fatal), DARTQ_CMD (the dartq side of the harness; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`), DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded to the harness
# drivers).
#
# Exit codes: 0 gate met (WARNING-tier Jaccard >= 1.0); 1 below the bar;
# 2 usage or environment error — missing binaries, fvm default missing, jq
# missing, unreadable report, violated vacuity guard — and the harness's own
# error tier (dartq crash, corpus preflight failure), which is never scored
# as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/m2-warning"
CODES_FILE="tests/harness/m2_warning_codes.txt"
THRESHOLD="1.0"
EXPECTED_VERSION="3.13.1"
OUT_ROOT="docs/audits/gate-m2"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="m2-r1"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_vacuous=0
label="${M2_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "m2-gate: --label needs a value" >&2
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
    echo "m2-gate: unknown argument '$1' (see scripts/m2-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$CODES_FILE" ]; then
  echo "m2-gate: $CODES_FILE not found (the frozen M2 code universe)" >&2
  exit 2
fi
# Frozen-universe count pin (#937): this file is the gate's denominator, and
# a hand-truncation during a window would otherwise pass with no runtime
# trace. Rows are counted exactly as the harness's parse_codes_file reads
# them (trim; skip blank and '#'-comment lines): the frozen M2 universe is
# 43 codes.
m2_count="$(grep -cv -e '^[[:space:]]*#' -e '^[[:space:]]*$' "$CODES_FILE")"
[ "$m2_count" -eq 43 ] || {
  echo "m2-gate: $CODES_FILE has $m2_count data rows, expected 43 — the frozen M2 universe was truncated or drifted" >&2
  exit 2
}
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "m2-gate: $CORPUS is not a corpus package — run: cargo run -p harness -- corpus" >&2
  exit 2
fi
if [ ! -d "$CORPUS/.dart_tool" ]; then
  echo "m2-gate: $CORPUS has no .dart_tool — pub get the corpus first (cd $CORPUS && dart pub get)" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "m2-gate: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "m2-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
if [ ! -x "$dart_bin" ]; then
  echo "m2-gate: fvm default dart not found at $dart_bin" >&2
  echo "  install the fvm default pin (dart 3.13.1) or point FVM_HOME at it;" >&2
  echo "  to use another oracle run the harness directly with --oracle-dart." >&2
  exit 2
fi

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "m2-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
if [ -e "$outdir" ]; then
  echo "m2-gate: $outdir already exists — refusing to overwrite recorded gate evidence (set a new M2_LABEL)" >&2
  exit 2
fi
echo "== m2-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s %s (WARNING tier, bar %s)\n' "codes" "$CODES_FILE" "$THRESHOLD"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter WARNING --threshold %s --codes-file %s --out %s\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$CODES_FILE" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version banner (run path only; the report meta records the real
# banner either way, and M2 semantics are pinned to the 3.13.1 caliber —
# a different oracle can legitimately emit a different code set).
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  echo "m2-gate: WARN expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}'"
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes on a WARNING-only run: 0 (verdict SKIPPED — the harness
# cannot judge this tier), 2 harness-own failure (dartq crash, corpus
# preflight — no verdict scored). A judged-FAIL exit 1 is structurally
# impossible here; the gate verdict comes from the WARNING-tier scoring
# below. The harness's ERROR-keyed non-vacuity guard is inert for the same
# reason; --allow-vacuous is still forwarded for its documentation value and
# honored in the guard below.
run_args=(
  run
  --corpus-dir "$CORPUS"
  --oracle-dart "$dart_bin"
  --dartq-cmd "$DARTQ_CMD"
  --severity-filter WARNING
  --threshold "$THRESHOLD"
  --codes-file "$CODES_FILE"
  --out "$outdir"
)
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] m2 gate"
mkdir -p "$outdir"
if "$HARNESS" "${run_args[@]}"; then
  echo "-- [$label] harness run complete; scoring the WARNING tier from the report"
else
  rc=$?
  echo "-- [$label] gate ERRORED (harness exit $rc; harness-own failure; no verdict scored)" >&2
  exit 2
fi

# --- score the WARNING tier ---------------------------------------------------
# Per-tier stats carry the gate metric; the ERROR-keyed report faces
# (passed: null, error_jaccard) are meaningless for a WARNING-only run.
row="$(jq -r '
  .severities[] | select(.severity == "WARNING") |
  [ (.jaccard * 1000 | round / 1000 | tostring),
    (.intersection | tostring),
    (.oracle_only | tostring),
    (.dartq_only | tostring)
  ] | @tsv' "$outdir/report.json" 2>/dev/null)"
ototal="$(jq -r '.oracle_total // 0' "$outdir/report.json" 2>/dev/null)"
dtotal="$(jq -r '.dartq_total // 0' "$outdir/report.json" 2>/dev/null)"
if [ -z "$row" ]; then
  echo "m2-gate: report $outdir/report.json is missing or unreadable, or carries no WARNING tier" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac inter oonly donly <<EOF
$row
EOF

# WARNING-tier vacuity guard (the harness's own guard is ERROR-keyed): zero
# oracle-side entries post-filter means the tier scored without judging
# anything. Oracle-side entries = intersection + oracle-only.
if [ "$((inter + oonly))" -eq 0 ]; then
  if [ "$allow_vacuous" -eq 1 ]; then
    echo "m2-gate: WARN vacuous run accepted (--allow-vacuous): no oracle-side WARNING entries post-filter"
  else
    echo "m2-gate: ERRORED — oracle side has no WARNING-tier entries post-filter; the gate would score without judging anything (--allow-vacuous accepts such a run)" >&2
    exit 2
  fi
fi

echo
echo "== m2-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (bar %s)\n' "warning_jaccard" "$jac" "$THRESHOLD"
printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
printf '  %-16s %s\n' "report" "$outdir/report.json"
# Exact decision on the raw counts: i/u >= 1.0 with u = intersection +
# oracle-only + dartq-only (the Jaccard union) means zero residuals —
# intersection == union, i.e. oracle-only + dartq-only == 0; integer, no
# rounding. The 3-decimal `jac` above is display-only: a true Jaccard in
# [0.9995, 1.0) rounds to 1.0 and must not pass.
union=$((inter + oonly + donly))
if [ "$inter" -ge "$union" ]; then
  echo "m2-gate: VERDICT PASS (WARNING-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "m2-gate: VERDICT FAIL (below the $THRESHOLD bar)"
exit 1
