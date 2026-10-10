#!/usr/bin/env bash
# m1-gate.sh — the M1 frozen-code differential gate (PRD section 20.3, M1 row).
#
# Runs the harness once over tests/corpus/m1-semantic with the frozen M1 code
# universe (tests/harness/m1_codes.txt), ERROR severity, threshold 0.95 — the
# M1 bar, deliberately below the M0 0.99 — against a single oracle of record:
# the fvm default install (Dart 3.13.1, the gate-caliber-r5 baseline). No
# matrix: M1 acceptance is defined on one caliber. The report lands under
# docs/audits/gate-m1/<label>/ (label: --label or M1_LABEL, default m1-r3;
# the recorded history is baseline-r1 and r2, and r2 measured ERROR-tier
# Jaccard 0.8235 below this bar over the pre-growth corpus — five dartq
# misses in the wrong_number_of_type_arguments family plus one dartq-only
# attribution).
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over the corpus. Never run it alongside development-phase lanes
# (memory-incident discipline). --dry-run executes nothing.
#
# Usage:
#   scripts/m1-gate.sh [--dry-run] [--label L] [--allow-vacuous]
#   scripts/m1-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact harness
#                   command, execute nothing (no dart, no harness).
#   --label L       report directory name under docs/audits/gate-m1/.
#                   Default: M1_LABEL env, else m1-r3.
#   --allow-vacuous forwarded to the harness: accept a run whose oracle side
#                   has no ERROR-tier entries post-filter. Without it such a
#                   run is a harness error — the gate would pass at 1.000
#                   without judging anything (a broken codes file, a wrong
#                   corpus, an oracle that silently produced nothing).
#
# Environment overrides: FVM_HOME (default ~/fvm; the default install must
# exist — its Dart version banner is checked on the run path, a mismatch is
# a WARN, not fatal), DARTQ_CMD (the dartq side of the harness; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>` — the recorded baseline-r2 shape), DARTQ_HARNESS_ORACLE_TIMEOUT_MS
# (forwarded to the harness drivers).
#
# Exit codes: 0 gate met (ERROR-tier Jaccard >= 0.95); 1 below the bar;
# 2 usage or environment error — missing binaries, fvm default missing, jq
# missing, unreadable report — and the harness's own error tier (exit 2:
# violated vacuity guard, dartq crash, corpus preflight failure), which is
# never scored as a Jaccard result. The harness SKIPPED verdict cannot
# occur here: the script always passes --severity-filter ERROR, so the gate
# tier is permanently in scope.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/m1-semantic"
CODES_FILE="tests/harness/m1_codes.txt"
THRESHOLD="0.95"
EXPECTED_VERSION="3.13.1"
OUT_ROOT="docs/audits/gate-m1"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="m1-r3"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_vacuous=0
label="${M1_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "m1-gate: --label needs a value" >&2
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
    echo "m1-gate: unknown argument '$1' (see scripts/m1-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$CODES_FILE" ]; then
  echo "m1-gate: $CODES_FILE not found (the frozen M1 code universe)" >&2
  exit 2
fi
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "m1-gate: $CORPUS is not a corpus package — run: cargo run -p harness -- corpus" >&2
  exit 2
fi
if [ ! -d "$CORPUS/.dart_tool" ]; then
  echo "m1-gate: $CORPUS has no .dart_tool — assemble + pub get the corpus first (see README)" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "m1-gate: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "m1-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
if [ ! -x "$dart_bin" ]; then
  echo "m1-gate: fvm default dart not found at $dart_bin" >&2
  echo "  install the fvm default pin (dart 3.13.1) or point FVM_HOME at it;" >&2
  echo "  to use another oracle run the harness directly with --oracle-dart." >&2
  exit 2
fi

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "m1-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
echo "== m1-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s %s (ERROR tier, bar %s)\n' "codes" "$CODES_FILE" "$THRESHOLD"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$CODES_FILE" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version banner (run path only; the report meta records the real
# banner either way, and M1 semantics are pinned to the 3.13.1 caliber —
# a different oracle can legitimately emit a different code set).
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  echo "m1-gate: WARN expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}'"
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes: 0 PASS, 1 below threshold (the report is still
# written), 2 harness error (vacuity guard, dartq crash, corpus preflight —
# no verdict is scored). The SKIPPED verdict cannot occur: ERROR is always
# in scope.
run_args=(
  run
  --corpus-dir "$CORPUS"
  --oracle-dart "$dart_bin"
  --dartq-cmd "$DARTQ_CMD"
  --severity-filter ERROR
  --threshold "$THRESHOLD"
  --codes-file "$CODES_FILE"
  --out "$outdir"
)
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] m1 gate"
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
  echo "m1-gate: report $outdir/report.json is missing or unreadable" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF
echo
echo "== m1-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (bar %s)\n' "error_jaccard" "$jac" "$THRESHOLD"
printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
printf '  %-16s %s\n' "report" "$outdir/report.json"
if [ "$verdict" = "PASS" ]; then
  echo "m1-gate: VERDICT PASS (ERROR-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "m1-gate: VERDICT FAIL (below the $THRESHOLD bar)"
exit 1
