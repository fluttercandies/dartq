#!/usr/bin/env bash
# lint-gate.sh — the LINT INFO-tier differential gate (design doc
# docs/plans/lint-differential-face.md section 3; ticket #721).
#
# Runs the harness once over tests/corpus/lints with the frozen r0 lint
# universe (tests/harness/lint_codes.txt, 88 names), INFO severity filtered
# to the LINT type, threshold 1.0 — full parity like every differential gate
# (2026-10-12 ruling, docs/facts/adr-2026-10-12-full-parity-bars.md;
# supersedes the PRD-era numeric bars; residuals pass only via individually
# adjudicated pinned exemptions, never a numeric budget) — against the same
# single oracle of record: the fvm default install (Dart 3.13.1, the
# gate-of-record banner check). Every package:lints rule emits at default severity INFO,
# so the lint face is exactly the INFO x LINT intersection of the machine
# rows.
#
# The harness's PASS/FAIL verdict, exit-code gate metric, and non-vacuity
# guard are hard-keyed to the ERROR tier, so an INFO-only run reports
# verdict SKIPPED and exits 0 by design. This script therefore scores the
# gate itself from the report's INFO-tier stats and runs the INFO-tier
# vacuity guard here — after the run, so unlike the ERROR tier a vacuous
# run has already materialized the report directory when the guard fires.
#
# Preflight additionally cross-checks the expected-divergence ledger
# (tests/harness/lint_divergences.tsv) against the frozen universe: every
# `excluded` ledger row must be absent from lint_codes.txt, every
# `in_universe` row present, and every vendored-set rule missing from
# lint_codes.txt must carry an `excluded` row — the ledger and the universe
# cannot drift (design doc section 4 consistency pin).
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq analyze`
# processes over the corpus. Never run it alongside development-phase lanes
# (memory-incident discipline). --dry-run executes nothing.
#
# Usage:
#   scripts/lint-gate.sh [--dry-run] [--label L] [--allow-vacuous]
#   scripts/lint-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact harness
#                   command, execute nothing (no dart, no harness).
#   --label L       report directory name under docs/audits/gate-lints/.
#                   Default: LINTS_LABEL env, else lints-r1.
#   --allow-vacuous forwarded to the harness and honored here: accept a run
#                   whose oracle side has no INFO-tier LINT entries
#                   post-filter. Without it such a run is a gate error —
#                   the tier would pass at 1.000 without judging anything
#                   (a broken codes file, an options-broken corpus, an
#                   oracle that silently produced nothing).
#
# Environment overrides: FVM_HOME (default ~/fvm), DARTQ_CMD (default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`), DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded).
#
# Exit codes: 0 gate met (INFO-tier Jaccard >= 1.0); 1 below the bar —
# the ratchet rule applies (fix the rule or exit it from lint_codes.txt
# with a ledger reason; never lower the bar); 2 usage or environment error
# — missing binaries, fvm default missing, jq missing, unreadable report,
# ledger/universe drift, violated vacuity guard — and the harness's own
# error tier, which is never scored as a Jaccard result.

set -u -o pipefail
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/lints"
CODES_FILE="tests/harness/lint_codes.txt"
LEDGER="tests/harness/lint_divergences.tsv"
THRESHOLD="1.0"
EXPECTED_VERSION="3.13.1"
OUT_ROOT="docs/audits/gate-lints"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="lints-r1"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_vacuous=0
label="${LINTS_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "lint-gate: --label needs a value" >&2
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
    echo "lint-gate: unknown argument '$1' (see scripts/lint-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$CODES_FILE" ]; then
  echo "lint-gate: $CODES_FILE not found (the frozen r0 lint universe)" >&2
  exit 2
fi
if [ ! -f "$LEDGER" ]; then
  echo "lint-gate: $LEDGER not found (the expected-divergence ledger)" >&2
  exit 2
fi
if [ ! -f "$CORPUS/analysis_options.yaml" ]; then
  echo "lint-gate: $CORPUS has no analysis_options.yaml — the lint face requires options-governed fixtures" >&2
  exit 2
fi
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "lint-gate: $CORPUS is not a corpus package" >&2
  exit 2
fi
if [ ! -f "$CORPUS/.dart_tool/package_config.json" ]; then
  echo "lint-gate: $CORPUS/.dart_tool/package_config.json missing — run dart pub get in the corpus root to regenerate it (safe: path deps only, offline); the tracked hand state is nested/.dart_tool/package_config.json and must never be regenerated" >&2
  exit 2
fi
if [ ! -f "$CORPUS/vendor/lints/lib/recommended.yaml" ]; then
  echo "lint-gate: $CORPUS/vendor/lints missing the vendored package:lints 5.1.1 copy" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "lint-gate: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "lint-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

# --- ledger/universe consistency (design doc section 4) -----------------------
# codes entries: one lowercase machine name per line, # comments.
codes_names() {
  grep -v '^#' "$CODES_FILE" | sed '/^[[:space:]]*$/d' | tr -d '\r'
}
# vendored set names: the list items of core.yaml + recommended.yaml,
# lowercased to the machine forms.
yaml_names() {
  cat "$CORPUS/vendor/lints/lib/core.yaml" "$CORPUS/vendor/lints/lib/recommended.yaml" |
    sed -n 's/^    - //p' | tr -d ' \r' | tr 'A-Z' 'a-z'
}
# ledger rows: rule<TAB>face<TAB>direction<TAB>gate_status<TAB>citation.
ledger_fields() {
  grep -v '^#' "$LEDGER" | sed '/^[[:space:]]*$/d' | tr -d '\r'
}

dupes="$(codes_names | sort | uniq -d)"
if [ -n "$dupes" ]; then
  echo "lint-gate: duplicate entries in $CODES_FILE:" >&2
  echo "$dupes" >&2
  exit 2
fi

bad_status=0
while IFS="$(printf '\t')" read -r l_rule l_face l_dir l_status l_cit; do
  case "$l_status" in
  excluded | in_universe | fixed) ;;
  *)
    echo "lint-gate: $LEDGER row '$l_rule' has gate_status '$l_status' (want excluded | in_universe | fixed)" >&2
    bad_status=1
    ;;
  esac
  case "$l_dir" in
  dartq_only | oracle_only | structural | both) ;;
  *)
    echo "lint-gate: $LEDGER row '$l_rule' has direction '$l_dir' (want dartq_only | oracle_only | structural | both)" >&2
    bad_status=1
    ;;
  esac
done <<EOF
$(ledger_fields)
EOF
if [ "$bad_status" -ne 0 ]; then
  exit 2
fi

drift=0
# A1: every `excluded` ledger row is absent from the codes file.
while IFS="$(printf '\t')" read -r l_rule _ _ l_status _; do
  [ "$l_status" = "excluded" ] || continue
  if codes_names | grep -qxF "$l_rule"; then
    echo "lint-gate: ledger drift — '$l_rule' is marked excluded but sits in $CODES_FILE" >&2
    drift=1
  fi
done <<EOF
$(ledger_fields)
EOF
# A2: every `in_universe` ledger row is present in the codes file.
while IFS="$(printf '\t')" read -r l_rule _ _ l_status _; do
  [ "$l_status" = "in_universe" ] || continue
  if ! codes_names | grep -qxF "$l_rule"; then
    echo "lint-gate: ledger drift — '$l_rule' is marked in_universe but is absent from $CODES_FILE" >&2
    drift=1
  fi
done <<EOF
$(ledger_fields)
EOF
# A3: every vendored-set rule absent from the codes file carries an
# `excluded` ledger row (the frozen exclusions are reasoned, not silent).
while read -r y_rule; do
  [ -n "$y_rule" ] || continue
  if codes_names | grep -qxF "$y_rule"; then
    continue
  fi
  if ! ledger_fields | awk -F '\t' '$4 == "excluded" { print $1 }' | grep -qxF "$y_rule"; then
    echo "lint-gate: ledger drift — '$y_rule' is outside $CODES_FILE but has no excluded row in $LEDGER" >&2
    drift=1
  fi
done <<EOF
$(yaml_names)
EOF
if [ "$drift" -ne 0 ]; then
  exit 2
fi

n_codes="$(codes_names | wc -l | tr -d ' ')"
n_yaml="$(yaml_names | wc -l | tr -d ' ')"

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
if [ ! -x "$dart_bin" ]; then
  echo "lint-gate: fvm default dart not found at $dart_bin" >&2
  echo "  install the fvm default pin (dart 3.13.1) or point FVM_HOME at it;" >&2
  echo "  to use another oracle run the harness directly with --oracle-dart." >&2
  exit 2
fi

DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "lint-gate: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
echo "== lint-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s %s (INFO x LINT tier, bar %s)\n' "codes" "$CODES_FILE" "$THRESHOLD"
printf '  %-16s %s codes entries, %s vendored set rules\n' "universe" "$n_codes" "$n_yaml"
printf '  %-16s %s\n' "ledger" "$LEDGER"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "report" "$outdir"
printf '  %-16s harness run --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter INFO --type-filter lint --threshold %s --codes-file %s --out %s\n' \
  "command" "$CORPUS" "$dart_bin" "$DARTQ_CMD" "$THRESHOLD" "$CODES_FILE" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# Oracle version banner (run path only; the report meta records the real
# banner either way, and lint semantics are pinned to the 3.13.1 caliber —
# a different oracle can legitimately emit a different code set).
found="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  echo "lint-gate: WARN expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}'"
fi

# --- run the gate ------------------------------------------------------------
# Harness exit codes on an INFO-only run: 0 (verdict SKIPPED — the harness
# cannot judge this tier), 2 harness-own failure (dartq crash, corpus
# preflight — no verdict scored). A judged-FAIL exit 1 is structurally
# impossible here; the gate verdict comes from the INFO-tier scoring below.
# The harness's ERROR-keyed non-vacuity guard is inert for the same reason;
# --allow-vacuous is still forwarded for its documentation value and
# honored in the guard below.
run_args=(
  run
  --corpus-dir "$CORPUS"
  --oracle-dart "$dart_bin"
  --dartq-cmd "$DARTQ_CMD"
  --severity-filter INFO
  --type-filter lint
  --threshold "$THRESHOLD"
  --codes-file "$CODES_FILE"
  --out "$outdir"
)
if [ "$allow_vacuous" -eq 1 ]; then
  run_args+=(--allow-vacuous)
fi

echo
echo "== [$label] lint gate"
mkdir -p "$outdir"
if "$HARNESS" "${run_args[@]}"; then
  echo "-- [$label] harness run complete; scoring the INFO tier from the report"
else
  rc=$?
  echo "-- [$label] gate ERRORED (harness exit $rc; harness-own failure; no verdict scored)" >&2
  exit 2
fi

# --- score the INFO tier -------------------------------------------------------
# Per-tier stats carry the gate metric; the ERROR-keyed report faces
# (passed: null, error_jaccard) are meaningless for an INFO-only run.
row="$(jq -r '
  .severities[] | select(.severity == "INFO") |
  [ (.jaccard * 1000 | round / 1000 | tostring),
    (.intersection | tostring),
    (.oracle_only | tostring),
    (.dartq_only | tostring)
  ] | @tsv' "$outdir/report.json" 2>/dev/null)"
ototal="$(jq -r '.oracle_total // 0' "$outdir/report.json" 2>/dev/null)"
dtotal="$(jq -r '.dartq_total // 0' "$outdir/report.json" 2>/dev/null)"
if [ -z "$row" ]; then
  echo "lint-gate: report $outdir/report.json is missing or unreadable, or carries no INFO tier" >&2
  exit 2
fi
IFS="$(printf '\t')" read -r jac inter oonly donly <<EOF
$row
EOF

# INFO-tier vacuity guard (the harness's own guard is ERROR-keyed): zero
# oracle-side LINT entries post-filter means the tier scored without
# judging anything. Oracle-side entries = intersection + oracle-only.
if [ "$((inter + oonly))" -eq 0 ]; then
  if [ "$allow_vacuous" -eq 1 ]; then
    echo "lint-gate: WARN vacuous run accepted (--allow-vacuous): no oracle-side INFO/LINT entries post-filter"
  else
    echo "lint-gate: ERRORED — oracle side has no INFO-tier LINT entries post-filter; the gate would score without judging anything (a violated options ancestry, a broken codes file, or a silent oracle; --allow-vacuous accepts such a run)" >&2
    exit 2
  fi
fi

echo
echo "== lint-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s (bar %s)\n' "info_jaccard" "$jac" "$THRESHOLD"
printf '  %-16s %s oracle | %s dartq (post-filter LINT instances)\n' "diags" "$ototal" "$dtotal"
printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json; triage against %s)\n' "diffs" "$oonly" "$donly" "$LEDGER"
printf '  %-16s %s\n' "report" "$outdir/report.json"
# Exact decision on the raw counts: i/u >= 1.0 with u = intersection +
# oracle-only + dartq-only (the Jaccard union) means zero residuals —
# intersection == union; integer, no rounding. The 3-decimal `jac` above is
# display-only: a true Jaccard in [0.9995, 1.0) rounds to 1.0 and must not
# pass.
union=$((inter + oonly + donly))
if [ "$inter" -ge "$union" ]; then
  echo "lint-gate: VERDICT PASS (INFO-tier Jaccard meets the $THRESHOLD bar)"
  exit 0
fi
echo "lint-gate: VERDICT FAIL (below the $THRESHOLD bar; ratchet: fix the rule or exit it from the universe with a ledger reason)"
exit 1
