#!/usr/bin/env bash
# m0-gate.sh — the M0 syntax + union differential gates.
#
# Two harness runs against a single oracle of record, the fvm default install:
#
#   Run A (syntax caliber, check face): tests/corpus/m0 +
#   tests/corpus/m0-syntax over the 255-code syntax universe
#   (tests/corpus/syntax-codes.txt), ERROR severity, bar 1.0 — the syntax
#   universe over the check face (`dartq check`, the scan+parse tier by
#   contract; no semantic diagnoser is mounted there).
#
#   Run B (union caliber, analyze face): the same corpora over the union of
#   the syntax universe with the frozen M1 code universe
#   (tests/harness/m1_codes.txt), pinned at exactly 290 codes = 255 + 36 - 1
#   overlap (the shared wrong_number_of_parameters_for_setter), ERROR
#   severity, bar 1.0 — the union universe over the analyze face (`dartq
#   analyze --sdk <fvm default dart-sdk>`, the engine tier with the semantic
#   diagnoser mounted). The check face can never match the oracle's
#   semantic rows, so a union-caliber run requires the analyze face.
#
#   Both bars are the 100% release caliber (ticket #1293; 2026-10-12 ruling,
#   the full-parity bars ADR; supersedes the earlier numeric bars):
#   each caliber passes only when its diff plane is empty after the
#   known-diffs exemption — every entry is either matched by
#   tests/corpus/regressions/known-diffs.json (the adjudicated burn-down
#   ledger, reported as EXEMPTED) or it is unattributed divergence and fails
#   the gate. There is no numeric pass bar; the raw (pre-exemption) Jaccard
#   is a TELEMETRY row.
#
# Neither run passes --type-filter: the M0 gates are code-scoped, never
# type-scoped.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze`, `dartq check` (run A)
# and `dartq analyze` (run B) processes over the corpora. Never run it
# alongside development-phase lanes
# (memory-incident discipline). --dry-run writes nothing and executes
# nothing: composing the union and counting it is pure local computation and
# runs on the dry-run path too, while the release-binary checks are run-path
# guards (a dry-run neither requires nor triggers a build).
#
# Usage:
#   scripts/m0-gate.sh --label L [--dry-run]
#   scripts/m0-gate.sh --dry-run        (plan only; --label optional there)
#   scripts/m0-gate.sh --help
#
#   --dry-run       resolve every path, compose the union in memory, print
#                   the plan (including the union count) and the exact
#                   harness commands, write nothing, execute nothing.
#   --label L       REQUIRED on the run path, no default: the report
#                   directories are target/evidence/gate-m0/<label>-syntax and
#                   target/evidence/gate-m0/<label>-union, and an existing
#                   directory is refused — the gate never overwrites
#                   recorded evidence. M0_LABEL env is accepted too.
#
# Environment overrides: FVM_HOME (default ~/fvm; the oracle version banner
# is enforced on the run path — a mismatch is FATAL; DARTQ_ORACLE_VERSION
# re-pins the expectation, DARTQ_ORACLE_VERSION_CHECK=off accepts a
# deliberate off-caliber oracle as a warning), DARTQ_CMD (run A's dartq
# side, the check face;
# default `./target/release/dartq check --format=json` — the recorded M0
# shape), DARTQ_ANALYZE_CMD (run B's dartq side, the analyze face; default
# `./target/release/dartq analyze --format=json --sdk <fvm default
# dart-sdk>` — the m1-gate shape against the release binary).
#
# Exit codes: 0 both calibers met the 100% caliber (diff planes empty after
# the known-diffs exemption); 1 either caliber carries unattributed
# divergence; 2 usage or environment error (missing binaries, corpora not
# pub-get, jq missing, label missing, an existing report directory, union
# drift off the pinned 290, oracle version mismatch, malformed known-diffs
# table), or the harness's own error tier
# (vacuity guard, dartq crash, corpus preflight failure) — the latter is
# never scored as a Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2
. "$(dirname "$0")/lib/diff-score.sh"

CORPUS_M0="tests/corpus/m0"
CORPUS_SYNTAX="tests/corpus/m0-syntax"
SYNTAX_CODES="tests/corpus/syntax-codes.txt"
M1_CODES="tests/harness/m1_codes.txt"
UNION_CODES="target/gate/union-codes.txt"
EXPECTED_UNION_COUNT="290" # 255 (syntax) + 36 (M1) - 1 overlap
SYNTAX_THRESHOLD="1.0"
UNION_THRESHOLD="1.0"
KNOWN_DIFFS="tests/corpus/regressions/known-diffs.json"
EXPECTED_VERSION="${DARTQ_ORACLE_VERSION:-3.13.1}"
OUT_ROOT="target/evidence/gate-m0"
HARNESS="./target/release/harness"
PERSIST_DIR="tests/corpus/regressions/cases"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
label="${M0_LABEL:-}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "m0-gate: --label needs a value" >&2
      exit 2
    }
    label="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "m0-gate: unknown argument '$1' (see scripts/m0-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# The label names the evidence directories: required on the run path, no
# default, ever — a silent default could overwrite recorded history. The
# dry-run path stays label-free via a placeholder.
if [ -z "$label" ] && [ "$dry_run" != 1 ]; then
  echo "m0-gate: --label is required (no default) — pass --label L or set M0_LABEL" >&2
  exit 2
fi
[ -n "$label" ] || label="<label>"

# --- preflight ---------------------------------------------------------------
# Pure-input checks run on both paths; the release-binary checks are
# run-path guards below (a dry-run must not fail — nor build — when the
# release harness is not on disk yet).
for path in "$SYNTAX_CODES" "$M1_CODES"; do
  [ -f "$path" ] || {
    echo "m0-gate: $path not found (a gate codes source)" >&2
    exit 2
  }
done
for corpus in "$CORPUS_M0" "$CORPUS_SYNTAX"; do
  [ -f "$corpus/pubspec.yaml" ] || {
    echo "m0-gate: $corpus is not a corpus package — run: cargo run -p harness -- corpus" >&2
    exit 2
  }
  [ -f "$corpus/.dart_tool/package_config.json" ] || {
    echo "m0-gate: $corpus has no .dart_tool/package_config.json — pub get the corpus first (cd $corpus && dart pub get)" >&2
    exit 2
  }
done
if ! command -v jq >/dev/null 2>&1; then
  echo "m0-gate: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"

# Run A drives the check face (scan+parse tier), run B the analyze face
# (engine tier with the semantic diagnoser) — the union universe includes
# the 36 semantic-tier codes, which only the analyze face can emit.
DEFAULT_CHECK_CMD="./target/release/dartq check --format=json"
CHECK_CMD="${DARTQ_CMD:-$DEFAULT_CHECK_CMD}"
DEFAULT_ANALYZE_CMD="./target/release/dartq analyze --format=json --sdk $sdk_root"
ANALYZE_CMD="${DARTQ_ANALYZE_CMD:-$DEFAULT_ANALYZE_CMD}"

# The union code universe: both sources comment-stripped, first field per
# line, whitespace stripped, empties dropped, case-fold deduplicated
# (sort -f -u) — pinned at exactly 290; any drift is a preflight failure,
# never a silent rescope of the gate.
compose_union() {
  sed -e 's/#.*//' "$SYNTAX_CODES" "$M1_CODES" |
    awk '{print $1}' |
    sed 's/[[:space:]]//g' |
    grep -v '^$' |
    sort -f -u
}
union_count="$(compose_union | wc -l | tr -d ' ')"
if [ "$union_count" -ne "$EXPECTED_UNION_COUNT" ]; then
  echo "m0-gate: union code universe drifted: expected $EXPECTED_UNION_COUNT = 255 ($SYNTAX_CODES) + 36 ($M1_CODES) - 1 overlap, found $union_count" >&2
  echo "  the M0 union gate is pinned to this exact universe; if a source changed deliberately, update the source and EXPECTED_UNION_COUNT together" >&2
  exit 2
fi

outdir_syntax="$OUT_ROOT/$label-syntax"
outdir_union="$OUT_ROOT/$label-union"
for d in "$outdir_syntax" "$outdir_union"; do
  if [ -e "$d" ]; then
    echo "m0-gate: $d already exists — refusing to overwrite recorded gate evidence (pass a new --label)" >&2
    exit 2
  fi
done

# --- plan --------------------------------------------------------------------
echo "== m0-gate plan"
printf '  %-16s %s + %s\n' "corpora" "$CORPUS_M0" "$CORPUS_SYNTAX"
printf '  %-16s %s (ERROR tier, raw threshold %s — telemetry; the gate is the 100% caliber)\n' "run A codes" "$SYNTAX_CODES" "$SYNTAX_THRESHOLD"
printf '  %-16s %s codes = 255 + 36 - 1 overlap (ERROR tier, raw threshold %s — telemetry)\n' "run B codes" "$union_count" "$UNION_THRESHOLD"
printf '  %-16s %s\n' "union file" "$UNION_CODES"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq run A" "$CHECK_CMD"
printf '  %-16s %s\n' "dartq run B" "$ANALYZE_CMD"
printf '  %-16s %s , %s\n' "reports" "$outdir_syntax" "$outdir_union"
printf '  %-16s harness run --corpus-dir %s --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions %s\n' \
  "run A" "$CORPUS_M0" "$CORPUS_SYNTAX" "$dart_bin" "$CHECK_CMD" "$SYNTAX_THRESHOLD" "$SYNTAX_CODES" "$outdir_syntax" "$PERSIST_DIR"
printf '  %-16s harness run --corpus-dir %s --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions %s\n' \
  "run B" "$CORPUS_M0" "$CORPUS_SYNTAX" "$dart_bin" "$ANALYZE_CMD" "$UNION_THRESHOLD" "$UNION_CODES" "$outdir_union" "$PERSIST_DIR"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, union composed in memory ($union_count codes), nothing written or executed"
  exit 0
fi

# Run-path binary guards (see preflight note above).
[ -x "$HARNESS" ] || {
  echo "m0-gate: $HARNESS not found — build it first: cargo build -p harness --release" >&2
  exit 2
}
[ -x "$dart_bin" ] || {
  echo "m0-gate: fvm default dart not found at $dart_bin (install the fvm default pin or set FVM_HOME)" >&2
  exit 2
}
# Both face defaults invoke the same release binary, so one -x guard covers
# run A's check shape and run B's analyze shape alike.
if { [ "$CHECK_CMD" = "$DEFAULT_CHECK_CMD" ] || [ "$ANALYZE_CMD" = "$DEFAULT_ANALYZE_CMD" ]; } && [ ! -x "./target/release/dartq" ]; then
  echo "m0-gate: ./target/release/dartq not found — build it first: cargo build -p dartq-cli --release (or set DARTQ_CMD / DARTQ_ANALYZE_CMD)" >&2
  exit 2
fi

# --- run the gates -----------------------------------------------------------
# Harness exit codes: 0 PASS, 1 below threshold (the report is still
# written), 2 harness error (vacuity guard, dartq crash, corpus preflight —
# no verdict is scored). The SKIPPED verdict cannot occur: ERROR is always
# in scope.

# Oracle version gate (run path only; --dry-run executes nothing). M0
# semantics are pinned to the same 3.13.1 caliber as the other differential
# gates — a different oracle can legitimately emit a different code set, so
# a mismatch is FATAL. Escapes: DARTQ_ORACLE_VERSION re-pins the
# expectation, DARTQ_ORACLE_VERSION_CHECK=off accepts a deliberate
# off-caliber oracle as a warning record.
banner="$("$dart_bin" --version 2>&1)"
found="$(printf '%s\n' "$banner" | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found" != "$EXPECTED_VERSION" ]; then
  if [ "${DARTQ_ORACLE_VERSION_CHECK:-}" = "off" ]; then
    echo "m0-gate: WARN dart $EXPECTED_VERSION mismatch accepted (DARTQ_ORACLE_VERSION_CHECK=off), found '${found:-unrecognized banner}'"
  else
    echo "m0-gate: FATAL expected dart $EXPECTED_VERSION, found '${found:-unrecognized banner}' — gate semantics are version-pinned; install the fvm default pin or point FVM_HOME at it, set DARTQ_ORACLE_VERSION to the oracle's version, or set DARTQ_ORACLE_VERSION_CHECK=off to accept a deliberate off-caliber oracle" >&2
    exit 2
  fi
fi
echo "== oracle banner: $banner"

mkdir -p "$(dirname "$UNION_CODES")"
compose_union >"$UNION_CODES"

verdict=PASS
run_gate() { # <threshold> <codes-file> <outdir> <dartq-cmd> <title>
  run_args=(
    run
    --corpus-dir "$CORPUS_M0"
    --corpus-dir "$CORPUS_SYNTAX"
    --oracle-dart "$dart_bin"
    --dartq-cmd "$4"
    --severity-filter ERROR
    --threshold "$1"
    --codes-file "$2"
 # Divergences land in tests/corpus/regressions/cases, exempted via known-diffs.json.
    --persist-regressions "$PERSIST_DIR"
    --out "$3"
  )
  echo
  echo "== [$label] m0 gate: $5 (100% caliber, ticket #1293)"
  # Capture the harness exit status at invocation: `if harness; then ... fi`
  # without an `else` exits 0 when the condition fails (the if statement's own
  # status), so a scored FAIL (exit 1) would be misread as 0 here.
  rc=0
  "$HARNESS" "${run_args[@]}" || rc=$?
  if [ "$rc" -ge 2 ]; then
    echo "-- [$label] $5 ERRORED (harness exit $rc, harness-own failure; no verdict scored)" >&2
    exit 2
  fi
  # rc 1 = below the RAW (pre-exemption) threshold — the harness verdict is
  # exemption-blind, so it is not the gate verdict; the 100% scoring below
  # decides. rc 0 means zero diffs at all (trivially fully attributed).
  if [ "$rc" -eq 0 ]; then
    echo "-- [$label] $5 raw bar met (zero diffs); report: $3/report.json"
  else
    echo "-- [$label] $5 below the raw bar (harness exit 1); scoring the known-diffs exemption from the report"
  fi
}

run_gate "$SYNTAX_THRESHOLD" "$SYNTAX_CODES" "$outdir_syntax" "$CHECK_CMD" "run A (syntax caliber, check face)"
run_gate "$UNION_THRESHOLD" "$UNION_CODES" "$outdir_union" "$ANALYZE_CMD" "run B (union caliber, $union_count codes, analyze face)"

# --- summary -----------------------------------------------------------------
score_caliber() { # <report-dir> <title> — prints telemetry rows, sets globals
  row="$(jq -r '
    [ (.error_jaccard * 1000 | round / 1000 | tostring),
      (.oracle_total | tostring),
      (.dartq_total | tostring),
      (.oracle_only | length | tostring),
      (.dartq_only | length | tostring)
    ] | @tsv' "$1/report.json" 2>/dev/null)"
  if [ -z "$row" ]; then
    echo "m0-gate: report $1/report.json is missing or unreadable" >&2
    exit 2
  fi
  IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF
  classification="$(diff_gate_classify "$1/report.json" "$KNOWN_DIFFS")"
  if [ -z "$classification" ]; then
    echo "m0-gate: cannot classify $1/report.json against $KNOWN_DIFFS (report unreadable or the known-diffs table malformed)" >&2
    exit 2
  fi
  IFS="$(printf '\t')" read -r _ exempted unattributed <<EOF
$(printf '%s\n' "$classification" | awk -F'\t' '$1 == "SUMMARY" {print; exit}')
EOF
  printf '  %-16s %s (TELEMETRY: raw, pre-exemption — never a gate)\n' "error_jaccard" "$jac"
  printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
  printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
  printf '  %-16s %s matched by the known-diffs ledger (the burn-down mechanism)\n' "exempted" "$exempted"
  printf '  %-16s %s (gate caliber: 0 — any unattributed divergence fails)\n' "unattributed" "$unattributed"
  if [ "$unattributed" -gt 0 ]; then
    printf '%s\n' "$classification" | awk -F'\t' '$1 == "UNATTRIBUTED" { printf "    %-12s %s %s\n", $2, $3, $4 }'
  fi
  printf '  %-16s %s\n' "report" "$1/report.json"
  if [ "$unattributed" -ne 0 ]; then
    verdict=FAIL
  fi
}

echo
echo "== m0-gate summary [$label]"
echo "  -- run A: syntax caliber"
score_caliber "$outdir_syntax" "run A"
echo "  -- run B: union caliber ($union_count codes)"
score_caliber "$outdir_union" "run B"

if [ "$verdict" = "PASS" ]; then
  echo "m0-gate: VERDICT PASS (100% caliber on both calibers: every diff entry matched or exempted)"
  exit 0
fi
echo "m0-gate: VERDICT FAIL (unattributed divergence — fix, or attribute in $KNOWN_DIFFS with a reason; the gate never passes on a numeric bar)"
exit 1
