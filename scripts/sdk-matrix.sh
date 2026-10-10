#!/usr/bin/env bash
# sdk-matrix.sh — multi-SDK-version differential gate (PRD §19.1/§19.2).
#
# Runs the existing gate caliber — the gate-caliber-r5 shape: `harness run`
# over tests/corpus/m0 + tests/corpus/m0-syntax, ERROR severity, threshold
# 0.99, the full syntax-codes TSV — once per supported Dart SDK oracle, then
# prints a per-version summary (version / Jaccard / verdict) and
# pre-classifies diff entries through the version-skew attribution rules
# (docs/facts/version-skew.md §5: the 7 `removedIn` codes and the
# language-version diagnostic family are version skew, everything else is a
# real-defect candidate). Classification is advisory; per rule the script
# never fixes anything — attribution reads the per-version report.
#
# SERIAL PHASE ONLY: every matrix entry spawns real `dart analyze` and
# `dartq check` processes over the full corpora. Never run this alongside
# development-phase lanes (memory-incident discipline).
#
# Matrix set (single source of truth: MATRIX + the vendored anchor below).
# The PRD §19.1 anchors are fvm 3.29 / 3.35 / 3.38 (Flutter versions; each
# bundles a Dart SDK) + the vendored dev SDK .third/dart-sdk (main
# 3.15.0-dev). The current gate oracle of record (fvm default → Dart 3.13.1,
# the gate-caliber-r5 baseline) is included so matrix numbers stay comparable
# to it. The dart versions below were read from each install's
# bin/cache/dart-sdk/VERSION, not assumed.
#
# Usage:
#   scripts/sdk-matrix.sh [--dry-run] [--dart-override PATH]
#   scripts/sdk-matrix.sh --help
#
#   --dry-run           resolve every matrix entry, print the resolved dart
#                       version, run no gate.
#   --dart-override P   run a single-SDK pass with the dart executable P as
#                       the oracle, report under docs/audits/sdk-matrix/
#                       override. This is the fallback when fvm is absent,
#                       and the way to cover the vendored main-3.15 anchor:
#                       a source snapshot ships no dart binary, so in a full
#                       matrix run that entry is SKIP unless a locally built
#                       main dart is supplied via VENDORED_DART=<path>.
#
# Environment overrides: FVM_HOME (default ~/fvm), DARTQ_CMD (the dartq side
# of the harness; default the prebuilt debug binary used by gate-caliber-r5),
# VENDORED_DART (see above).
#
# Exit codes: 0 every executed gate PASSes (SKIP entries are recorded in the
# summary, not fatal); 1 at least one gate FAILs or errors; 2 usage or
# environment error (missing binaries, jq, fvm without override).

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

# --- matrix definition (single source of truth) ------------------------------
# "label|fvm version dir under $FVM_HOME/versions ('default' = the fvm default
# symlink)|expected dart version". The first three entries are the PRD §19.1
# anchors; dart-3.13 is the oracle of record (gate-caliber-r5).
MATRIX=(
  "dart-3.7|3.29.3|3.7.2"
  "dart-3.9|3.35.0|3.9.0"
  "dart-3.10|3.38.1|3.10.0"
  "dart-3.13|default|3.13.1"
)
VENDORED_LABEL="dart-main-3.15" # .third/dart-sdk (main 3.15.0-dev anchor)

CORPORA=(tests/corpus/m0 tests/corpus/m0-syntax)
CODES_FILE="crates/dartq-diagnostics/data/syntax-codes.tsv"
THRESHOLD="0.99"
OUT_ROOT="docs/audits/sdk-matrix"
HARNESS="./target/debug/harness"
DEFAULT_DARTQ_CMD="./target/debug/dartq check --format=machine"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"

# Version-skew diagnostic families (docs/facts/version-skew.md §2.2 removedIn
# codes + §2 language-version family), matched case-insensitively against the
# report.json oracle_only/dartq_only entry codes.
SKEW_RE='^(multiple_variables_in_for_each|invalid_operator_for_super|non_user_definable_operator|missing_terminator_for_parameter_group|wrong_terminator_for_parameter_group|unexpected_terminator_for_parameter_group|unable_get_content|invalid_language_version_override.*|illegal_language_version_override.*|inconsistent_language_version_override.*|language_version_too_high_explicit|experiment_not_enabled.*)$'

usage() {
  sed -n '2,47p' "$0" | sed 's/^# \{0,1\}//'
}

dry_run=0
dart_override=""
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --dart-override)
    [ $# -ge 2 ] || {
      echo "sdk-matrix: --dart-override needs a path" >&2
      exit 2
    }
    dart_override="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "sdk-matrix: unknown argument '$1' (see scripts/sdk-matrix.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -x "$HARNESS" ]; then
  echo "sdk-matrix: $HARNESS not found — build it first: cargo build -p harness" >&2
  exit 2
fi
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  echo "sdk-matrix: ./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd> to point elsewhere)" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "sdk-matrix: jq is required for the summary (brew install jq)" >&2
  exit 2
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"

fvm_dart() { # $1 = fvm version dir name or "default"; prints the dart path
  local candidate
  if [ "$1" = "default" ]; then
    candidate="$FVM_HOME/default/bin/cache/dart-sdk/bin/dart"
  else
    candidate="$FVM_HOME/versions/$1/bin/cache/dart-sdk/bin/dart"
    if [ ! -x "$candidate" ]; then
      # Legacy fvm v2 layout: versions directly under $FVM_HOME.
      candidate="$FVM_HOME/$1/bin/cache/dart-sdk/bin/dart"
    fi
  fi
  [ -x "$candidate" ] && printf '%s' "$candidate"
}

# --- resolve the run plan ----------------------------------------------------
# Plan entries: "label|dart-path|skip-reason" (dart path empty = SKIP).
plan=()
if [ -n "$dart_override" ]; then
  if [ ! -x "$dart_override" ]; then
    echo "sdk-matrix: --dart-override '$dart_override' is not an executable" >&2
    exit 2
  fi
  plan+=("override|$dart_override|")
else
  if [ ! -d "$FVM_HOME" ]; then
    echo "sdk-matrix: fvm is not installed (nothing under $FVM_HOME)." >&2
    echo "  Install it per https://fvm.app/documentation/getting-started/installation" >&2
    echo "  and add the anchors: fvm install 3.29.3 3.35.0 3.38.1 — or rerun with" >&2
    echo "  --dart-override <path-to-dart> to run a single-SDK pass against an" >&2
    echo "  existing local Dart SDK." >&2
    exit 2
  fi
  for entry in "${MATRIX[@]}"; do
    label="${entry%%|*}"
    fvm_ver="$(printf '%s' "$entry" | cut -d'|' -f2)"
    if dart_bin="$(fvm_dart "$fvm_ver")"; then
      plan+=("$label|$dart_bin|")
    else
      plan+=("$label||skipped: fvm $fvm_ver not installed under $FVM_HOME")
    fi
  done
  # Vendored dev anchor: the source snapshot ships no dart binary.
  if [ -n "${VENDORED_DART:-}" ]; then
    if [ -x "$VENDORED_DART" ]; then
      plan+=("$VENDORED_LABEL|$VENDORED_DART|")
    else
      plan+=("$VENDORED_LABEL||skipped: VENDORED_DART='$VENDORED_DART' is not executable")
    fi
  else
    plan+=("$VENDORED_LABEL||skipped: .third/dart-sdk is a source snapshot (no dart binary); build dart from it and rerun with VENDORED_DART=<path> or --dart-override <path>")
  fi
fi

echo "== sdk-matrix run plan"
resolved=0
for item in "${plan[@]}"; do
  IFS='|' read -r label dart_bin skip_reason <<EOF
$item
EOF
  if [ -n "$skip_reason" ]; then
    printf '  %-16s %s\n' "$label" "$skip_reason"
  else
    printf '  %-16s %s -> ' "$label" "$dart_bin"
    "$dart_bin" --version
    resolved=$((resolved + 1))
  fi
done
if [ "$dry_run" = 1 ]; then
  echo "== dry run: $resolved entries resolved, no gate executed"
  exit 0
fi
if [ "$resolved" -eq 0 ]; then
  echo "sdk-matrix: no oracle resolved — nothing to run" >&2
  exit 2
fi

# --- run the gate per version ------------------------------------------------
# Gate caliber = gate-caliber-r5: ERROR severity, threshold 0.99, full syntax
# codes TSV, m0 + m0-syntax corpora. Harness exit codes: 0 PASS (or SKIPPED
# verdict), 1 below gate (the report is still written), 2 harness error.
results=()
for item in "${plan[@]}"; do
  IFS='|' read -r label dart_bin skip_reason <<EOF
$item
EOF
  if [ -n "$skip_reason" ]; then
    results+=("$label|SKIP|-|$skip_reason")
    continue
  fi
  outdir="$OUT_ROOT/$label"
  echo
  echo "== [$label] oracle $dart_bin"
  mkdir -p "$outdir"
  corpus_args=()
  for c in "${CORPORA[@]}"; do
    corpus_args+=(--corpus-dir "$c")
  done
  if "$HARNESS" run \
    "${corpus_args[@]}" \
    --oracle-dart "$dart_bin" \
    --dartq-cmd "$DARTQ_CMD" \
    --severity-filter ERROR \
    --threshold "$THRESHOLD" \
    --codes-file "$CODES_FILE" \
    --out "$outdir"; then
    echo "-- [$label] gate PASSED; report: $outdir/report.json"
    results+=("$label|PASS|-|$outdir/report.json")
  else
    rc=$?
    echo "-- [$label] gate FAILED (harness exit $rc); report: $outdir/report.json"
    results+=("$label|FAIL|-|$outdir/report.json")
  fi
done

# --- summary -----------------------------------------------------------------
echo
echo "== sdk-matrix summary"
printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
  "version" "dart" "jaccard" "result" "oracle-only" "dartq-only" "report"
printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
  "-------" "----" "-------" "------" "-----------" "----------" "------"
any_fail=0
for item in "${results[@]}"; do
  IFS='|' read -r label verdict filler report <<EOF
$item
EOF
  if [ "$verdict" = "SKIP" ]; then
    printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
      "$label" "-" "-" "SKIP" "-" "-" "$report"
    continue
  fi
  # TSV: dart version, jaccard, "O-skew/O-total", "D-skew/D-total".
  row="$(jq -r --arg re "$SKEW_RE" '
    [ (.meta.oracle_version | capture("^Dart SDK version: (?<v>[^ ]+)") | .v),
      (.error_jaccard * 1000 | round / 1000),
      ([.oracle_only[].code | ascii_downcase | select(test($re))] | length | tostring) + "/" + (.oracle_only | length | tostring),
      ([.dartq_only[].code | ascii_downcase | select(test($re))] | length | tostring) + "/" + (.dartq_only | length | tostring)
    ] | @tsv' "$report" 2>/dev/null)"
  if [ -z "$row" ]; then
    printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
      "$label" "?" "?" "$verdict" "?" "?" "$report (unreadable)"
    any_fail=1
    continue
  fi
  IFS="$(printf '\t')" read -r dver jac oskew dskew <<EOF
$row
EOF
  [ "$verdict" != "PASS" ] && any_fail=1
  printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
    "$label" "$dver" "$jac" "$verdict" "$oskew" "$dskew" "$report"
done

echo
echo "skew columns: '<skew-classified>/<total>' per side (docs/facts/version-skew.md"
echo "§5 rule 3: removedIn codes + language-version family = version skew, not dartq"
echo "defects; the remaining oracle-only/dartq-only entries are real-defect candidates"
echo "— attribute each through the per-version report.json; no automatic fixing)."
if [ "$any_fail" = 1 ]; then
  echo "sdk-matrix: VERDICT FAIL (at least one version below gate or errored)"
  exit 1
fi
echo "sdk-matrix: VERDICT PASS (all executed versions meet the gate)"
