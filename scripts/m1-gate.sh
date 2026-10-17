#!/usr/bin/env bash
# m1-gate.sh — the M1 frozen-code differential gate.
#
# Runs the harness once over tests/corpus/m1-semantic with the frozen M1 code
# universe (tests/harness/m1_codes.txt), ERROR severity, threshold 1.0 — the
# 100% release caliber (ticket #1293; 2026-10-12 ruling, the full-parity bars
# ADR; supersedes the earlier numeric bars): the gate passes only
# when the ERROR-tier diff plane is empty after the known-diffs exemption —
# every entry is either matched by tests/corpus/regressions/known-diffs.json
# (the adjudicated burn-down ledger, reported as EXEMPTED) or it is
# unattributed divergence and fails the gate. There is no numeric pass bar;
# the raw (pre-exemption) Jaccard is a TELEMETRY row. Oracle of record: the
# fvm default install (Dart 3.13.1, the gate-caliber-r5 baseline). No
# matrix: M1 acceptance is defined on one caliber. The report lands under
# target/evidence/gate-m1/<label>/ (label: --label or M1_LABEL, default m1-r4;
# the recorded history is baseline-r1, r2 and r3, and r2 measured ERROR-tier
# Jaccard 0.8235 below the raw bar over the pre-growth corpus — five dartq
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
#   --label L       report directory name under target/evidence/gate-m1/.
#                   Default: M1_LABEL env, else m1-r4. An existing report
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
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>` — the recorded baseline-r2 shape), DARTQ_HARNESS_ORACLE_TIMEOUT_MS
# (forwarded to the harness drivers).
#
# Exit codes: 0 gate met (ERROR-tier diff plane empty after the known-diffs
# exemption — set equality, ticket #1293); 1 unattributed divergence (some
# diff entry is neither matched nor exempted); 2 usage or environment error —
# missing binaries, fvm default missing, jq missing, unreadable report,
# malformed known-diffs table — and the harness's own error tier (exit 2:
# violated vacuity guard, dartq crash, corpus preflight failure), which is
# never scored as a Jaccard result. The harness SKIPPED verdict cannot
# occur here: the script always passes --severity-filter ERROR, so the gate
# tier is permanently in scope.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2
. "$(dirname "$0")/lib/diff-score.sh"

CORPUS="tests/corpus/m1-semantic"
CODES_FILE="tests/harness/m1_codes.txt"
THRESHOLD="1.0"
KNOWN_DIFFS="tests/corpus/regressions/known-diffs.json"
EXPECTED_VERSION="${DARTQ_ORACLE_VERSION:-3.13.1}"
OUT_ROOT="target/evidence/gate-m1"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="m1-r4"

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
# Frozen-universe count pin (#937): this file is the gate's denominator, and
# a hand-truncation during a window would otherwise pass with no runtime
# trace. Rows are counted exactly as the harness's parse_codes_file reads
# them (trim; skip blank and '#'-comment lines): the frozen M1 universe is
# 36 codes (m0's union pin cross-checks this from the other side).
m1_count="$(grep -cv -e '^[[:space:]]*#' -e '^[[:space:]]*$' "$CODES_FILE")"
[ "$m1_count" -eq 36 ] || {
  echo "m1-gate: $CODES_FILE has $m1_count data rows, expected 36 — the frozen M1 universe was truncated or drifted" >&2
  exit 2
}
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
if [ -e "$outdir" ]; then
  echo "m1-gate: $outdir already exists — refusing to overwrite recorded gate evidence (pass a new --label)" >&2
  exit 2
fi
echo "== m1-gate plan"
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

# Oracle version gate (run path only; the report meta records the real
# banner either way). M1 semantics are pinned to the 3.13.1 caliber — a
# different oracle can legitimately emit a different code set, so a
# mismatch is FATAL. Escapes: DARTQ_ORACLE_VERSION re-pins the expectation,
# DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate off-caliber oracle as
# a warning record.
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  if [ "${DARTQ_ORACLE_VERSION_CHECK:-}" = "off" ]; then
    echo "m1-gate: WARN dart $EXPECTED_VERSION mismatch accepted (DARTQ_ORACLE_VERSION_CHECK=off), found '${found:-unrecognized banner}'"
  else
    echo "m1-gate: FATAL expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}' — gate semantics are version-pinned; install the fvm default pin or point FVM_HOME at it, set DARTQ_ORACLE_VERSION to the oracle's version, or set DARTQ_ORACLE_VERSION_CHECK=off to accept a deliberate off-caliber oracle" >&2
    exit 2
  fi
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes: 0 the RAW (pre-exemption) ERROR Jaccard met the
# threshold, 1 below it (the report is still written either way), 2 harness
# error (vacuity guard, dartq crash, corpus preflight — no verdict scored).
# The harness verdict is exemption-blind, so rc 1 is NOT the gate verdict:
# the 100% scoring below classifies every diff entry against the known-diffs
# ledger and fails only on unattributed divergence. The SKIPPED verdict
# cannot occur: ERROR is always in scope.
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
echo "== [$label] m1 gate"
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
  echo "m1-gate: report $outdir/report.json is missing or unreadable" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF

# 100% scoring (ticket #1293): classify every flat diff entry against the
# known-diffs ledger. Empty output = unreadable report or malformed table.
classification="$(diff_gate_classify "$outdir/report.json" "$KNOWN_DIFFS")"
if [ -z "$classification" ]; then
  echo "m1-gate: cannot classify $outdir/report.json against $KNOWN_DIFFS (report unreadable or the known-diffs table malformed)" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r _ exempted unattributed <<EOF
$(printf '%s\n' "$classification" | awk -F'\t' '$1 == "SUMMARY" {print; exit}')
EOF

echo
echo "== m1-gate summary"
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
  echo "m1-gate: VERDICT PASS (100% caliber: every ERROR-tier diff entry is matched or exempted)"
  exit 0
fi
echo "m1-gate: VERDICT FAIL ($unattributed unattributed divergence entries — fix, or attribute in $KNOWN_DIFFS with a reason; the gate never passes on a numeric bar)"
exit 1
