#!/usr/bin/env bash
# sdk-matrix.sh — multi-SDK-version differential gate (PRD §19.1/§19.2).
#
# Runs the existing gate caliber — the gate-caliber-r5 shape: `harness run`
# over tests/corpus/m0 + tests/corpus/m0-syntax, ERROR severity, threshold
# 1.0 (full parity like every differential gate; 2026-10-12 ruling,
# the full-parity bars ADR), the full syntax-codes
# TSV — once per supported Dart SDK oracle, then
# prints a per-version summary (version / Jaccard / verdict) and
# pre-classifies one-sided (code,file) entries through the version-skew
# attribution rules (the version-skew ledger §5: the 7 `removedIn` codes
# and the language-version diagnostic family are version skew; so is the
# addedIn class — dartq-only entries in a feature family whose rung in
# scripts/sdk-feature-ladder.txt postdates the oracle's language minor.
# dartq is frozen at 3.15-main semantics, so a feature enabled by default
# above the oracle can never be matched by it; without the addedIn class
# every version step below a feature's release manufactures false defect
# candidates. Everything else is a real-defect candidate). Classification
# reads raw-outputs.json —
# the unfiltered plane — so codes the gate's codes-file set drops (the
# removedIn codes are pruned from the TSV) are still classified.
# Classification is advisory; per rule the script never fixes anything —
# attribution reads the per-version report.
#
# Sweep scope: the syntax/m0 caliber ONLY (m0 + m0-syntax, syntax codes
# TSV). The lint/m2/m3 gates stay single-version at the gate of record
# (dart 3.13.1) until a per-version rule-existence filter exists: an SDK
# older than a rule's introduction does not know the rule name, so a lint
# matrix run degenerates into option-validation faces and hollow
# (both-sides-skip) gates. Per-version clamp expectations live in
# scripts/sdk-matrix-expectations.tsv; its UNKNOWN-until-probed rows are
# filled by the serial probe phase (see that file's header for the probe
# contract).
#
# SERIAL PHASE ONLY: every matrix entry spawns real `dart analyze` and
# `dartq check` processes over the full corpora. Never run this alongside
# development-phase lanes (memory-incident discipline).
#
# Matrix set (single source of truth: MATRIX + the vendored anchor below).
# The PRD §19.1 anchors are fvm 3.29 / 3.35 / 3.38 (Flutter versions; each
# bundles a Dart SDK) + the vendored dev SDK source checkout (main
# 3.15.0-dev). The current gate oracle of record (fvm default → Dart 3.13.1,
# the gate-caliber-r5 baseline) is included so matrix numbers stay comparable
# to it. The dart versions below were read from each install's
# bin/cache/dart-sdk/VERSION, not assumed.
#
# Usage:
#   scripts/sdk-matrix.sh [--dry-run] [--dart-override PATH] [--allow-skip]
#                         [--check-expectations]
#   scripts/sdk-matrix.sh --help
#
#   --dry-run           resolve every matrix entry, print the resolved dart
#                       version, run no gate.
#   --check-expectations
#                       validate the committed data tables
#                       (scripts/sdk-feature-ladder.txt +
#                       scripts/sdk-matrix-expectations.tsv: row shape,
#                       ascending rungs, minor cross-references, duplicate
#                       keys) and exit 0/2. Static text validation only —
#                       runs no oracle, no version resolution, no binaries;
#                       needs neither the harness nor jq.
#   --dart-override P   run a single-SDK pass with the dart executable P as
#                       the oracle, report under target/evidence/sdk-matrix/
#                       override. This is the fallback when fvm is absent,
#                       and the way to cover the vendored main-3.15 anchor:
#                       a source snapshot ships no dart binary, so in a full
#                       matrix run that entry is SKIP unless a locally built
#                       main dart is supplied via VENDORED_DART=<path>.
#   --allow-skip        accept SKIPped matrix entries (an install-coverage
#                       gap) as non-fatal: the run can still exit 0 when all
#                       executed gates PASS. Without it, any SKIPped entry
#                       makes the run exit 2 (verdict INCOMPLETE) even when
#                       every executed gate PASSes.
#
# Environment overrides: FVM_HOME (default ~/fvm), DARTQ_CMD (the dartq side
# of the harness; default the prebuilt debug binary used by gate-caliber-r5),
# HARNESS (default ./target/debug/harness; the release profiles built by the
# CI matrix workflows override it to ./target/release/harness), VENDORED_DART
# (see above).
#
# Exit codes: 0 every executed gate PASSes and every matrix entry resolved
# (or --allow-skip accepted the SKIPs); 1 at least one gate FAILed below the
# threshold; 2 usage or environment error — missing binaries, jq, fvm without
# override, a SKIPped entry without --allow-skip (verdict INCOMPLETE), or an
# entry whose harness run errored (harness rc >= 2) while none failed the
# gate (verdict ERRORED). A resolved dart whose version banner contradicts
# its MATRIX expectation is marked WARN in the plan and summary, not fatal.

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
VENDORED_LABEL="dart-main-3.15" # local SDK source checkout (main 3.15.0-dev anchor)

CORPORA=(tests/corpus/m0 tests/corpus/m0-syntax)
CODES_FILE="crates/dartq-diagnostics/data/syntax-codes.tsv"
LADDER_FILE="scripts/sdk-feature-ladder.txt"
EXPECT_FILE="scripts/sdk-matrix-expectations.tsv"
THRESHOLD="1.0"
OUT_ROOT="target/evidence/sdk-matrix"
HARNESS="${HARNESS:-./target/debug/harness}"
DEFAULT_DARTQ_CMD="./target/debug/dartq check --format=machine"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"

# Version-skew diagnostic families (the version-skew ledger §2.2 removedIn
# codes + §2 language-version family), matched case-insensitively against the
# one-sided (code,file) entry codes computed from raw-outputs.json — the
# unfiltered plane. report.json's diff lists are post-codes-filter and can
# never contain the removedIn codes (they are pruned from the TSV), so the
# raw plane is the only place they can be classified as skew.
SKEW_RE='^(multiple_variables_in_for_each|invalid_operator_for_super|non_user_definable_operator|missing_terminator_for_parameter_group|wrong_terminator_for_parameter_group|unexpected_terminator_for_parameter_group|unable_get_content|invalid_language_version_override.*|illegal_language_version_override.*|inconsistent_language_version_override.*|language_version_too_high_explicit|experiment_not_enabled.*)$'

# addedIn attribution source: the committed feature-release ladder (rung =
# language minor at which the feature is enabled by default). Loaded once
# into a jq JSON array; the per-version regex union is computed inside the
# summary's jq program from the report's oracle version banner. `-` rungs
# become an empty regex and are dropped by the jq program. The data files
# are shape-checked before anything runs (check_data_tables), so a malformed
# row fails fast instead of silently degrading the classification.
#
# Expectation rows are keyed by oracle language minor, so they cross-check
# against the pinned oracle snapshot (scripts/sdk-versions.txt, read-only),
# NOT the feature rungs — clamp faces exist at every below-rung oracle minor
# (e.g. 3.5 has no feature rung but its digit-separator faces flip).
# stdout = the problems (empty when clean); exit status 0 clean / 1 problems.
check_data_tables() {
  awk -F'|' '
    FILENAME == ARGV[1] {
      if ($0 ~ /^[[:space:]]*(#|$)/) next
      split($1, p, ".")
      vminor[p[2]] = 1
      next
    }
    FILENAME == ARGV[2] {
      if ($0 ~ /^[[:space:]]*(#|$)/) next
      if ($1 !~ /^[0-9]+$/) {
        printf "ladder %s:%d: bad minor field (want <minor>|<feature>|<code-names-or-dash>): %s\n", FILENAME, FNR, $0
        bad = 1
        next
      }
      if (NF != 3) {
        printf "ladder %s:%d: want exactly <minor>|<feature>|<code-names-or-dash> (code names are comma-separated): %s\n", FILENAME, FNR, $0
        bad = 1
        next
      }
      if (prev != "" && $1 + 0 < prev + 0) {
        printf "ladder %s:%d: minors must be ascending: %s\n", FILENAME, FNR, $0
        bad = 1
      }
      if ($3 != "-" && (index($3, "\"") || index($3, "\\"))) {
        printf "ladder %s:%d: code names must not contain quotes or backslashes: %s\n", FILENAME, FNR, $0
        bad = 1
      }
      prev = $1
      next
    }
    {
      if ($0 ~ /^[[:space:]]*(#|$)/) next
      if (NF != 4) {
        printf "expect %s:%d: want <minor-or-*>|<kind>|<face>|<expectation>: %s\n", FILENAME, FNR, $0
        bad = 1
        next
      }
      if ($1 != "*" && $1 !~ /^[0-9]+$/) {
        printf "expect %s:%d: bad minor field (integer or *): %s\n", FILENAME, FNR, $0
        bad = 1
        next
      }
      if ($1 != "*" && !($1 in vminor)) {
        printf "expect %s:%d: minor is not an oracle minor in scripts/sdk-versions.txt: %s\n", FILENAME, FNR, $0
        bad = 1
      }
      if ($2 !~ /^(clamp|lint|sdk-constraint)$/) {
        printf "expect %s:%d: bad kind (clamp|lint|sdk-constraint): %s\n", FILENAME, FNR, $0
        bad = 1
      }
      if ($4 != "silent" && $4 != "fires" && $4 != "UNKNOWN-until-probed" && $4 !~ /^recodes:/) {
        printf "expect %s:%d: bad expectation (silent|fires|recodes:<codes>|UNKNOWN-until-probed): %s\n", FILENAME, FNR, $0
        bad = 1
      }
      key = $1 SUBSEP $2 SUBSEP $3
      if (key in seen) {
        printf "expect %s:%d: duplicate minor/kind/face row: %s\n", FILENAME, FNR, $0
        bad = 1
      }
      seen[key] = 1
    }
    END { exit bad ? 1 : 0 }
  ' scripts/sdk-versions.txt "$LADDER_FILE" "$EXPECT_FILE"
}

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
allow_skip=0
check_expect=0
dart_override=""
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --check-expectations) check_expect=1 ;;
  --dart-override)
    [ $# -ge 2 ] || {
      echo "sdk-matrix: --dart-override needs a path" >&2
      exit 2
    }
    dart_override="$2"
    shift
    ;;
  --allow-skip) allow_skip=1 ;;
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

# --- committed data tables (load + shape-check) -------------------------------
# Runs before every other preflight so --check-expectations needs neither
# the harness, dartq, fvm, nor jq — it is pure text validation.
for f in "$LADDER_FILE" "$EXPECT_FILE"; do
  if [ ! -f "$f" ]; then
    echo "sdk-matrix: committed data table missing: $f (see scripts/sdk-feature-ladder.txt and scripts/sdk-matrix-expectations.tsv in a clean checkout)" >&2
    exit 2
  fi
done
if ! table_errs="$(check_data_tables)"; then
  [ -n "$table_errs" ] && printf '%s\n' "$table_errs" >&2
  echo "sdk-matrix: committed data tables invalid ($LADDER_FILE / $EXPECT_FILE)" >&2
  exit 2
fi
if [ "$check_expect" = 1 ]; then
  rungs=$(grep -Ev '^[[:space:]]*(#|$)' "$LADDER_FILE" | wc -l | tr -d ' ')
  rows=$(grep -Ev '^[[:space:]]*(#|$)' "$EXPECT_FILE" | wc -l | tr -d ' ')
  unknown=$(grep -Ev '^[[:space:]]*(#|$)' "$EXPECT_FILE" | grep -c 'UNKNOWN-until-probed' || true)
  echo "sdk-matrix: data tables OK — $LADDER_FILE: $rungs rungs; $EXPECT_FILE: $rows rows, $unknown UNKNOWN-until-probed (serial probe phase fills them)"
  exit 0
fi
# The ladder as jq input: [{"v":<minor>,"re":"<joined ERE>"}, ...]. `-` rungs
# become an empty re and are dropped by the summary's jq program.
ladder_json="[$(awk -F'|' '
  /^[[:space:]]*(#|$)/ { next }
  {
    re = ($3 == "-") ? "" : $3
    gsub(/[[:space:]]+/, "", re)
    gsub(/,/, "|", re) # the column lists names comma-separated; join to ERE
    printf "%s{\"v\":%s,\"re\":\"%s\"}", sep, $1, re
    sep = ","
  }
' "$LADDER_FILE")]"

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

# Advisory version check (MATRIX field 3): a WARN note when the resolved
# dart's version banner does not carry the expected version — the row label
# may not mean what it says. Not fatal; empty output = no warning.
version_warn() { # $1 = dart path, $2 = expected version
  local found
  found="$("$1" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
  if [ -z "$found" ]; then
    printf 'WARN: version banner unrecognized (expected dart %s)' "$2"
  elif [ "$found" != "$2" ]; then
    printf 'WARN: expected dart %s, found %s' "$2" "$found"
  fi
}

# --- resolve the run plan ----------------------------------------------------
# Plan entries: "label|dart-path|skip-reason|warn" (dart path empty = SKIP;
# warn empty = version check clean or not applicable).
plan=()
if [ -n "$dart_override" ]; then
  if [ ! -x "$dart_override" ]; then
    echo "sdk-matrix: --dart-override '$dart_override' is not an executable" >&2
    exit 2
  fi
  plan+=("override|$dart_override||")
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
    expected="$(printf '%s' "$entry" | cut -d'|' -f3)"
    if dart_bin="$(fvm_dart "$fvm_ver")"; then
      plan+=("$label|$dart_bin||$(version_warn "$dart_bin" "$expected")")
    else
      plan+=("$label||skipped: fvm $fvm_ver not installed under $FVM_HOME|")
    fi
  done
  # Vendored dev anchor: the source snapshot ships no dart binary.
  if [ -n "${VENDORED_DART:-}" ]; then
    if [ -x "$VENDORED_DART" ]; then
      plan+=("$VENDORED_LABEL|$VENDORED_DART||")
    else
      plan+=("$VENDORED_LABEL||skipped: VENDORED_DART='$VENDORED_DART' is not executable|")
    fi
  else
    plan+=("$VENDORED_LABEL||skipped: the local SDK source checkout is a source snapshot (no dart binary); build dart from it and rerun with VENDORED_DART=<path> or --dart-override <path>|")
  fi
fi

echo "== sdk-matrix run plan"
resolved=0
for item in "${plan[@]}"; do
  IFS='|' read -r label dart_bin skip_reason warn <<EOF
$item
EOF
  if [ -n "$skip_reason" ]; then
    printf '  %-16s %s\n' "$label" "$skip_reason"
  else
    printf '  %-16s %s -> ' "$label" "$dart_bin"
    "$dart_bin" --version
    if [ -n "$warn" ]; then
      printf '  %s\n' "$warn"
    fi
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
# Gate caliber = gate-caliber-r5: ERROR severity, threshold 1.0, full syntax
# codes TSV, m0 + m0-syntax corpora. Harness exit codes: 0 PASS (or SKIPPED
# verdict), 1 below gate (the report is still written), 2 harness error.
results=()
skips=0
for item in "${plan[@]}"; do
  IFS='|' read -r label dart_bin skip_reason warn <<EOF
$item
EOF
  if [ -n "$skip_reason" ]; then
    results+=("$label|SKIP|-|$skip_reason")
    skips=$((skips + 1))
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
    if [ "$rc" -eq 1 ]; then
      echo "-- [$label] gate FAILED below the threshold (harness exit 1); report: $outdir/report.json"
      results+=("$label|FAIL|-|$outdir/report.json")
    else
      echo "-- [$label] gate ERRORED (harness exit $rc, harness-own failure); report may be absent: $outdir/report.json"
      results+=("$label|ERROR|-|$outdir/report.json")
    fi
  fi
done

# --- summary -----------------------------------------------------------------
echo
echo "== sdk-matrix summary"
printf 'resolved=%d/%d\n' "$resolved" "${#plan[@]}"
printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
  "version" "dart" "jaccard" "result" "oracle-only" "dartq-only" "report"
printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
  "-------" "----" "-------" "------" "-----------" "----------" "------"
any_fail=0
any_error=0
for item in "${results[@]}"; do
  IFS='|' read -r label verdict filler report <<EOF
$item
EOF
  if [ "$verdict" = "SKIP" ]; then
    printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
      "$label" "-" "-" "SKIP" "-" "-" "$report"
    continue
  fi
  # TSV: dart version, jaccard, "O-skew/O-total", "D-skew/D-addedIn/D-total" —
  # skew classified over one-sided (code,file) entries computed from the RAW
  # plane (raw-outputs.json, all severities, unfiltered); report.json still
  # carries the version banner and the gate metric. The dartq-only addedIn
  # regex union is the set of feature-ladder rungs strictly above the
  # oracle's language minor (banner-derived; an unparseable banner disables
  # addedIn attribution — conservative, keeps entries as defect candidates).
  # An ERROR row's report may be absent (harness errors exit before the
  # comparison writes one).
  rawfile="$(dirname "$report")/raw-outputs.json"
  row="$(jq -rs --arg re "$SKEW_RE" --argjson ladder "$ladder_json" '
    .[0] as $r | .[1] as $raw |
    ($raw.oracle | [.corpora[].diagnostics[] | {c: .code, f: .file}] | unique) as $oe |
    ($raw.dartq  | [.corpora[].diagnostics[] | {c: .code, f: .file}] | unique) as $de |
    ($oe - $de) as $oo |
    ($de - $oe) as $do |
    ($r.meta.oracle_version | capture("^Dart SDK version: (?<v>[^ ]+)") | .v) as $v |
    (try ($v | split(".") | .[1] | tonumber) catch 999) as $min |
    ([$ladder[] | select(.v > $min) | select(.re != "") | .re] | join("|")) as $a |
    (if $a == "" then "^$" else $a end) as $are |
    [ $v,
      ($r.error_jaccard * 1000 | round / 1000),
      ([$oo[].c | ascii_downcase | select(test($re))] | length | tostring) + "/" + ($oo | length | tostring),
      ([$do[].c | ascii_downcase | select(test($re))] | length | tostring) + "/" +
      ([$do[].c | ascii_downcase | select(test($are))] | length | tostring) + "/" +
      ($do | length | tostring)
    ] | @tsv' "$report" "$rawfile" 2>/dev/null)"
  if [ -z "$row" ]; then
    printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
      "$label" "?" "?" "$verdict" "?" "?" "$report (unreadable)"
    if [ "$verdict" = "FAIL" ]; then
      any_fail=1
    else
      any_error=1
    fi
    continue
  fi
  IFS="$(printf '\t')" read -r dver jac oskew dskew <<EOF
$row
EOF
  case "$verdict" in
  FAIL) any_fail=1 ;;
  ERROR) any_error=1 ;;
  esac
  printf '%-16s %-10s %8s %-5s %16s %16s %s\n' \
    "$label" "$dver" "$jac" "$verdict" "$oskew" "$dskew" "$report"
done

echo
echo "resolved=$resolved/${#plan[@]} (SKIP entries count as unresolved)"
echo "oracle-only column: '<removedIn-skew>/<total>'; dartq-only column:"
echo "'<removedIn-skew>/<addedIn>/<total>' — one-sided (code,file) entries"
echo "computed from raw-outputs.json (all severities, unfiltered), so codes"
echo "outside the gate's codes-file set — e.g. the removedIn codes pruned"
echo "from the TSV — are still classified. removedIn codes + language-version"
echo "family = version skew (the version-skew ledger §5 rule 3); dartq-only"
echo "entries in a feature family whose rung in scripts/sdk-feature-ladder.txt"
echo "postdates the oracle's language minor = addedIn skew (the oracle cannot"
echo "emit them); the remaining one-sided entries are real-defect candidates —"
echo "attribute each through the per-version report.json + raw-outputs.json;"
echo "no automatic fixing. Per-version clamp expectations:"
echo "scripts/sdk-matrix-expectations.tsv (UNKNOWN-until-probed rows are"
echo "filled by the serial probe phase)."
if [ "$any_fail" = 1 ]; then
  echo "sdk-matrix: VERDICT FAIL (at least one version below the gate)"
  exit 1
fi
if [ "$any_error" = 1 ]; then
  echo "sdk-matrix: VERDICT ERRORED (harness/environment error on at least one entry, none below the gate)"
  exit 2
fi
if [ "$skips" -gt 0 ] && [ "$allow_skip" -eq 0 ]; then
  echo "sdk-matrix: VERDICT INCOMPLETE ($skips entry/skipped; rerun with --allow-skip to accept the coverage gap)"
  exit 2
fi
echo "sdk-matrix: VERDICT PASS (all resolved versions meet the gate)"
exit 0
