#!/usr/bin/env bash
# torture-gate.sh — the semantic torture differential gate (ticket #1290).
#
# Runs the harness once over tests/corpus/torture/corpus (the generated
# complexity-axis corpus, tests/corpus/torture/README.md) with ERROR
# severity, threshold 1.0 — the full-parity bar shared by every differential
# gate — against the single oracle of record: the fvm default install (Dart
# 3.13.1, the gate-caliber baseline). Unlike the m1/m2/sem2 tiers there is
# no frozen codes universe: the torture corpus is calibrated by the oracle
# itself, so EVERY ERROR code the oracle emits is judged. The generated
# errprobe package (a clearly separated error-probe subtree) guarantees the
# oracle side is non-vacuous, so the gate refuses --allow-vacuous by default
# and only forwards it when explicitly asked.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over ~2k corpus files. Never run it alongside development-phase
# lanes (memory-incident discipline). --dry-run executes nothing.
#
# Usage:
#   scripts/torture-gate.sh [--dry-run] [--label L] [--allow-vacuous]
#   scripts/torture-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact harness
#                   command, execute nothing (no dart, no harness).
#   --label L       report directory name under target/evidence/gate-torture/.
#                   Default: TORTURE_LABEL env, else torture-r1. An existing
#                   report directory is refused — a rerun never overwrites
#                   recorded evidence.
#   --allow-vacuous forwarded to the harness: accept a run whose oracle side
#                   has no ERROR-tier entries post-filter. Without it such a
#                   run is a harness error — the errprobe subtree must always
#                   put ERROR entries on the oracle side; vacuity there means
#                   the corpus was not generated or was gutted.
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
# dart-sdk>` — the recorded m1-gate shape), DARTQ_HARNESS_ORACLE_TIMEOUT_MS
# (forwarded to the harness drivers; the ~2k-file torture tree may exceed
# the 120 s default, so this script raises it to 600000 unless already set).
#
# Exit codes: 0 gate met (ERROR-tier Jaccard >= 1.0); 1 below the bar;
# 2 usage or environment error — missing binaries, fvm default missing, jq
# missing, corpus not generated — and the harness's own error tier (exit 2:
# violated vacuity guard, dartq crash, corpus preflight failure), which is
# never scored as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/torture/corpus"
GENERATOR="tests/corpus/torture/tools/gen_torture.py"
THRESHOLD="1.0"
EXPECTED_VERSION="${DARTQ_ORACLE_VERSION:-3.13.1}"
OUT_ROOT="target/evidence/gate-torture"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="torture-r1"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_vacuous=0
label="${TORTURE_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "torture-gate: --label needs a value" >&2
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
    echo "torture-gate: unknown argument '$1' (see scripts/torture-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$GENERATOR" ]; then
  echo "torture-gate: $GENERATOR not found (the deterministic corpus generator)" >&2
  exit 2
fi
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "torture-gate: $CORPUS is not generated — run:" >&2
  echo "  python3 $GENERATOR --force" >&2
  exit 2
fi
if [ ! -f "$CORPUS/.dart_tool/package_config.json" ]; then
  echo "torture-gate: $CORPUS has no .dart_tool/package_config.json — regenerate:" >&2
  echo "  python3 $GENERATOR --force" >&2
  exit 2
fi
if [ ! -d "$CORPUS/packages/errprobe" ]; then
  echo "torture-gate: $CORPUS/packages/errprobe missing — the error-probe subtree keeps the oracle side non-vacuous; regenerate:" >&2
  echo "  python3 $GENERATOR --force" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "torture-gate: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "torture-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
if [ ! -x "$dart_bin" ]; then
  echo "torture-gate: fvm default dart not found at $dart_bin" >&2
  echo "  install the fvm default pin (dart 3.13.1) or point FVM_HOME at it;" >&2
  echo "  to use another oracle run the harness directly with --oracle-dart." >&2
  exit 2
fi

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=json --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "torture-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
if [ -e "$outdir" ]; then
  echo "torture-gate: $outdir already exists — refusing to overwrite recorded gate evidence (pass a new --label)" >&2
  exit 2
fi
echo "== torture-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s ERROR tier over every code, bar %s (no frozen universe)\n' "scope" "$THRESHOLD"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --out %s --persist-regressions tests/corpus/regressions/cases\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version gate (run path only; the report meta records the real
# banner either way). The modifier/pattern faces are pinned to the 3.13.1
# caliber — a different oracle can legitimately emit a different code set,
# so a mismatch is FATAL. Escapes: DARTQ_ORACLE_VERSION re-pins the
# expectation, DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate
# off-caliber oracle as a warning record.
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  if [ "${DARTQ_ORACLE_VERSION_CHECK:-}" = "off" ]; then
    echo "torture-gate: WARN dart $EXPECTED_VERSION mismatch accepted (DARTQ_ORACLE_VERSION_CHECK=off), found '${found:-unrecognized banner}'"
  else
    echo "torture-gate: FATAL expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}' — gate semantics are version-pinned; install the fvm default pin or point FVM_HOME at it, set DARTQ_ORACLE_VERSION to the oracle's version, or set DARTQ_ORACLE_VERSION_CHECK=off to accept a deliberate off-caliber oracle" >&2
    exit 2
  fi
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
  # The torture tree is ~2k files of dense semantics; give the oracle driver
  # the extended budget the mega leg uses (script default, env-overridable).
  --out "$outdir"
  # Divergences land in tests/corpus/regressions/cases, exempted via tests/corpus/regressions/known-diffs.json.
  --persist-regressions tests/corpus/regressions/cases
)
if [ -z "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS:-}" ]; then
  export DARTQ_HARNESS_ORACLE_TIMEOUT_MS=600000
fi
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] torture gate"
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
  echo "torture-gate: report $outdir/report.json is missing or unreadable" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF
echo
echo "== torture-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (bar %s, all ERROR codes)\n' "error_jaccard" "$jac" "$THRESHOLD"
printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
printf '  %-16s %s\n' "report" "$outdir/report.json"
if [ "$verdict" = "PASS" ]; then
  echo "torture-gate: VERDICT PASS (ERROR-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "torture-gate: VERDICT FAIL (below the $THRESHOLD bar)"
exit 1
