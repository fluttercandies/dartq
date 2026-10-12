#!/usr/bin/env bash
# m0-gate.sh — the M0 syntax + union differential gates (PRD section 20.3, M0 row).
#
# Two harness runs against a single oracle of record, the fvm default install:
#
#   Run A (syntax caliber): tests/corpus/m0 + tests/corpus/m0-syntax over the
#   255-code syntax universe (tests/corpus/syntax-codes.txt), ERROR severity,
#   bar 0.99.
#
#   Run B (union caliber): the same corpora over the union of the syntax
#   universe with the frozen M1 code universe (tests/harness/m1_codes.txt),
#   pinned at exactly 290 codes = 255 + 36 - 1 overlap (the shared
#   wrong_number_of_parameters_for_setter), ERROR severity, bar 0.95.
#
# Neither run passes --type-filter: the M0 gates are code-scoped, never
# type-scoped.
#
# SERIAL PHASE ONLY: a run spawns real `dart analyze` and `dartq check`
# processes over the corpora. Never run it alongside development-phase lanes
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
#                   directories are docs/audits/gate-m0/<label>-syntax and
#                   docs/audits/gate-m0/<label>-union, and an existing
#                   directory is refused — the gate never overwrites
#                   recorded evidence. M0_LABEL env is accepted too.
#
# Environment overrides: FVM_HOME (default ~/fvm; the oracle version banner
# is echoed on the run path), DARTQ_CMD (the dartq side of the harness;
# default `./target/release/dartq check --format=machine` — the recorded M0
# shape).
#
# Exit codes: 0 both calibers met their bars; 1 either caliber below its
# bar; 2 usage or environment error (missing binaries, corpora not pub-get,
# jq missing, label missing, an existing report directory, union drift off
# the pinned 290), or the harness's own error tier (vacuity guard, dartq
# crash, corpus preflight failure) — the latter is never scored as a
# Jaccard result.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS_M0="tests/corpus/m0"
CORPUS_SYNTAX="tests/corpus/m0-syntax"
SYNTAX_CODES="tests/corpus/syntax-codes.txt"
M1_CODES="tests/harness/m1_codes.txt"
UNION_CODES="target/gate/union-codes.txt"
EXPECTED_UNION_COUNT="290" # 255 (syntax) + 36 (M1) - 1 overlap
SYNTAX_THRESHOLD="0.99"
UNION_THRESHOLD="0.95"
OUT_ROOT="docs/audits/gate-m0"
HARNESS="./target/release/harness"
PERSIST_DIR="tests/corpus/regressions/cases"
DEFAULT_DARTQ_CMD="./target/release/dartq check --format=machine"

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

DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"

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
printf '  %-16s %s (ERROR tier, bar %s)\n' "run A codes" "$SYNTAX_CODES" "$SYNTAX_THRESHOLD"
printf '  %-16s %s codes = 255 + 36 - 1 overlap (ERROR tier, bar %s)\n' "run B codes" "$union_count" "$UNION_THRESHOLD"
printf '  %-16s %s\n' "union file" "$UNION_CODES"
printf '  %-16s %s\n' "oracle" "$dart_bin"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s , %s\n' "reports" "$outdir_syntax" "$outdir_union"
printf '  %-16s harness run --corpus-dir %s --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions %s\n' \
  "run A" "$CORPUS_M0" "$CORPUS_SYNTAX" "$dart_bin" "$DARTQ_CMD" "$SYNTAX_THRESHOLD" "$SYNTAX_CODES" "$outdir_syntax" "$PERSIST_DIR"
printf '  %-16s harness run --corpus-dir %s --corpus-dir %s --oracle-dart %s --dartq-cmd "%s" --severity-filter ERROR --threshold %s --codes-file %s --out %s --persist-regressions %s\n' \
  "run B" "$CORPUS_M0" "$CORPUS_SYNTAX" "$dart_bin" "$DARTQ_CMD" "$UNION_THRESHOLD" "$UNION_CODES" "$outdir_union" "$PERSIST_DIR"
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
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/release/dartq" ]; then
  echo "m0-gate: ./target/release/dartq not found — build it first: cargo build -p dartq-cli --release (or set DARTQ_CMD=<cmd>)" >&2
  exit 2
fi

# --- run the gates -----------------------------------------------------------
# Harness exit codes: 0 PASS, 1 below threshold (the report is still
# written), 2 harness error (vacuity guard, dartq crash, corpus preflight —
# no verdict is scored). The SKIPPED verdict cannot occur: ERROR is always
# in scope.

# Oracle version banner (run path only; --dry-run executes nothing).
echo "== oracle banner: $("$dart_bin" --version 2>&1)"

mkdir -p "$(dirname "$UNION_CODES")"
compose_union >"$UNION_CODES"

verdict=PASS
run_gate() { # <threshold> <codes-file> <outdir> <title>
  run_args=(
    run
    --corpus-dir "$CORPUS_M0"
    --corpus-dir "$CORPUS_SYNTAX"
    --oracle-dart "$dart_bin"
    --dartq-cmd "$DARTQ_CMD"
    --severity-filter ERROR
    --threshold "$1"
    --codes-file "$2"
    # Divergences land in tests/corpus/regressions/cases, exempted via tests/corpus/regressions/known-diffs.json (PRD §19.1).
    --persist-regressions "$PERSIST_DIR"
    --out "$3"
  )
  echo
  echo "== [$label] m0 gate: $4 (bar $1)"
  if "$HARNESS" "${run_args[@]}"; then
    echo "-- [$label] $4 PASSED the bar; report: $3/report.json"
    return 0
  fi
  rc=$?
  if [ "$rc" -eq 1 ]; then
    verdict=FAIL
    echo "-- [$label] $4 FAILED below the bar (harness exit 1); report: $3/report.json"
  else
    echo "-- [$label] $4 ERRORED (harness exit $rc, harness-own failure; no verdict scored)" >&2
    exit 2
  fi
}

run_gate "$SYNTAX_THRESHOLD" "$SYNTAX_CODES" "$outdir_syntax" "run A (syntax caliber)"
run_gate "$UNION_THRESHOLD" "$UNION_CODES" "$outdir_union" "run B (union caliber, $union_count codes)"

# --- summary -----------------------------------------------------------------
summarize() { # <report-dir> <bar>
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
  printf '  %-16s %s (bar %s)\n' "error_jaccard" "$jac" "$2"
  printf '  %-16s %s oracle | %s dartq (post-filter instances)\n' "diags" "$ototal" "$dtotal"
  printf '  %-16s %s oracle-only, %s dartq-only entries (attribution: report.json)\n' "diffs" "$oonly" "$donly"
  printf '  %-16s %s\n' "report" "$1/report.json"
}

echo
echo "== m0-gate summary [$label]"
echo "  -- run A: syntax caliber"
summarize "$outdir_syntax" "$SYNTAX_THRESHOLD"
echo "  -- run B: union caliber ($union_count codes)"
summarize "$outdir_union" "$UNION_THRESHOLD"

if [ "$verdict" = "PASS" ]; then
  echo "m0-gate: VERDICT PASS (both calibers meet their bars: $SYNTAX_THRESHOLD syntax, $UNION_THRESHOLD union)"
  exit 0
fi
echo "m0-gate: VERDICT FAIL (a caliber is below its bar; see the summary above)"
exit 1
