#!/usr/bin/env bash
# incremental-gate.sh — the incremental-consistency gate (every incremental
# step must match a fresh full recompute byte-wise) over the curated
# edit-sequence corpus (ticket #1082; serves the frozen #147 verification
# order).
#
# For every sequence under tests/corpus/incremental/ (one directory per
# sequence: a minimal pure-Dart package plus edits.jsonl), the script runs
# `harness replay`: ONE live `dartq daemon` session applies the sequence
# through overlay didChange + the `dartq/analyze` pull (the incremental
# face) while the same edits land on a materialized disk copy re-analyzed
# fresh per step (the full-recompute face). Per step both diagnostic sets
# normalize to one canonical shape and compare byte-wise; a sequence is
# green only when every step matches. Any non-zero replay exit (1
# mismatch, 2 harness error) or a report that does not record a pass fails
# the gate with the failing sequence's name.
#
# Because tests/corpus/*/.dart_tool/ is gitignored (machine-specific
# absolute paths), each sequence package is materialized into a temp copy
# where this script writes the pub-generated files the analyzer faces
# require (pubspec.lock, .dart_tool/package_config.json) — the same
# hand-written shape scripts/schema-freeze-sweep.sh uses for its daemon
# fixture. The committed corpus stays pubspec.yaml + sources + edits.jsonl.
#
# A sequence directory may instead carry a one-line `base` file naming a
# repo-relative workspace the sequence edits (tests/corpus/complex, the
# generated tests/corpus/torture/corpus). For those the gate points
# --corpus-dir straight at that workspace and skips materialization: replay
# copies the corpus before any edit lands, so the base tree is never
# written, but its analyzer inputs (pubspec.yaml,
# .dart_tool/package_config.json) must already exist — the preflight dies
# with a remedy when they do not.
#
# --negative-control proves the gate can actually fail. It runs the first
# sequence to a green baseline, then injects three deliberate corruptions
# and asserts each trips a real failure path:
#   verdict-flip   the passing report is copied with verdict.passed
#                  flipped to false — the gate's report verdict check
#                  (the same verify_report the gate loop uses) must
#                  reject it;
#   input-corrupt  the first edits step's line is moved far out of range —
#                  replay must exit non-zero and the gate's exit-code
#                  check must score it failed;
#   vacuous        a genuinely clean package driven by a no-op edit makes
#                  every step compare zero diagnostics on BOTH faces —
#                  the harness's sequence-level non-vacuity guard must
#                  fail the run (passed=false naming the vacuity, replay
#                  exit 1), because all-equal on empty sets proves
#                  nothing. The package is real (a live daemon session and
#                  fresh batch analyses), so the guard — not an incidental
#                  error — is what must trip: the report must carry the
#                  vacuity failure reason.
# The control exits 0 only when all three injections were detected; a
# control that passes anyway is itself a gate defect (exit 1). Control
# artifacts stay in a temp dir and never touch recorded evidence.
#
# SERIAL PHASE ONLY: a run spawns real dartq processes (one daemon session
# and one fresh full analysis per step, per sequence). Never run it
# alongside development-phase lanes (memory-incident discipline).
#
# Usage:
#   scripts/incremental-gate.sh [--label L] [--negative-control] [--help]
#
#   --label L           evidence directory name under
#                       target/evidence/incremental/ (one replay report
#                       subdirectory per sequence). Default:
#                       INCREMENTAL_LABEL env, else incr-r1. An existing
#                       directory refuses to run — recorded evidence is
#                       never overwritten (bump the label).
#   --negative-control  run the fault-injection self-check instead of the
#                       gate loop (writes no evidence).
#
# Environment overrides: DARTQ_BIN (default ./target/release/dartq — both
# replay faces drive it), HARNESS_BIN (default ./target/release/harness),
# DARTQ_INCR_SDK (default $DART_SDK, else ~/fvm/default/bin/cache/dart-sdk —
# passed to both faces as --sdk).
#
# Requires: jq. No network.
#
# Exit codes: 0 every sequence green (or the negative control detected all
# three injections); 1 at least one sequence diverged or errored — the
# failing sequence name is printed — or the negative control failed to
# trip; 2 environment error — missing tool/binary/SDK, missing corpus or
# sequence, or existing evidence directory.

set -u -o pipefail
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 2

CORPUS_ROOT="tests/corpus/incremental"
OUT_ROOT="target/evidence/incremental"
DARTQ_BIN="${DARTQ_BIN:-./target/release/dartq}"
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
SDK="${DARTQ_INCR_SDK:-${DART_SDK:-$HOME/fvm/default/bin/cache/dart-sdk}}"
label="${INCREMENTAL_LABEL:-incr-r1}"
# Adjudicated-skip sequences (comma-separated dir names under CORPUS_ROOT).
# Every name here carries an open engine ticket whose fix un-skips it; the
# skip is a documented convergence-debt disposition (the trend-w5-B
# pattern), never a silent pass. Current: none — the relink face (#2100)
# landed (set-moving batches re-resolve affected directive holders), so
# recovery-missing-uri-relink runs live again.
SKIP_SEQUENCES="${DARTQ_INCR_SKIP:-}"
negative_control=0

usage() {
  sed -n '2,/^set -u -o pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() { # <message...> — environment error
  echo "incremental-gate: $*" >&2
  exit 2
}

fail() { # <message...> — gate verdict failure
  echo "incremental-gate: $*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
  --label)
    [ $# -ge 2 ] || die "--label needs a value"
    label="$2"
    shift
    ;;
  --negative-control) negative_control=1 ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/incremental-gate.sh --help)"
    ;;
  esac
  shift
done

# --- preflight ----------------------------------------------------------------

command -v jq >/dev/null 2>&1 || die "jq is required (brew install jq)"
[ -x "$HARNESS_BIN" ] || die "$HARNESS_BIN not found — build it first (cargo build --release -p harness)"
[ -x "$DARTQ_BIN" ] || die "$DARTQ_BIN not found — build it first (cargo build --release -p dartq-cli)"
[ -d "$SDK" ] || die "SDK root not found at $SDK — set DARTQ_INCR_SDK or DART_SDK"

[ -d "$CORPUS_ROOT" ] || die "$CORPUS_ROOT not found (the curated edit-sequence corpus)"

# Sequence dirs = immediate subdirectories carrying edits.jsonl, collected
# into the positional parameters (bash 3.2: no arrays of paths with safe
# quoting otherwise). A no-match glob stays literal and the -f skips it.
# Adjudicated skips (SKIP_SEQUENCES) drop here with one banner line each —
# visible in every gate log, so a skip can never ride silently.
set --
for dir in "$CORPUS_ROOT"/*/; do
  [ -f "${dir}edits.jsonl" ] || continue
  name="$(basename "$dir")"
  case ",$SKIP_SEQUENCES," in
    *",$name,"*)
      printf -- '-- [incremental-gate] SKIP sequence %s (adjudicated: %s; see SKIP_SEQUENCES)\n' \
        "$name" "$SKIP_SEQUENCES"
      continue
      ;;
  esac
  set -- "$@" "$dir"
done
[ $# -ge 1 ] || die "no edit sequences under $CORPUS_ROOT (each sequence dir needs edits.jsonl)"

# Materialize one sequence package into <dest>: copy the committed tree,
# then write the pub-generated files the analyzer faces require. The
# package name comes from the committed pubspec.yaml — the single source
# of truth the self package: imports resolve against.
materialize_sequence() { # <seq_dir> <dest>
  local seq="$1" dest="$2" name
  [ -f "$seq/pubspec.yaml" ] || die "$seq has no pubspec.yaml"
  name="$(sed -n 's/^name: *//p' "$seq/pubspec.yaml")"
  [ -n "$name" ] || die "$seq/pubspec.yaml carries no name"
  mkdir -p "$dest"
  cp -R "$seq/." "$dest/" || die "copying $seq failed"
  cat >"$dest/pubspec.lock" <<EOF
packages: {}
sdks:
  dart: ">=3.5.0 <4.0.0"
EOF
  mkdir -p "$dest/.dart_tool"
  cat >"$dest/.dart_tool/package_config.json" <<EOF
{"configVersion": 2, "packages": [{"name": "$name", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}
EOF
}

# Resolve the corpus one sequence replays against: its own materialized
# copy, or — when the sequence directory carries a one-line `base` file —
# the named workspace directly (replay copies the corpus before any edit
# lands, so the base tree is never written).
resolve_corpus() { # <seq_dir> <materialize_dest> — echoes the corpus dir
  local seq="$1" dest="$2" base
  if [ ! -f "$seq/base" ]; then
    materialize_sequence "$seq" "$dest"
    printf '%s\n' "$dest"
    return
  fi
  base="$(tr -d '[:space:]' <"$seq/base")"
  [ -n "$base" ] || die "$seq/base: empty base path"
  [ -d "$base" ] || die "$base (from $seq/base) not found — restore or generate the base corpus first"
  [ -f "$base/pubspec.yaml" ] || die "$base has no pubspec.yaml"
  [ -f "$base/.dart_tool/package_config.json" ] ||
    die "$base has no .dart_tool/package_config.json — materialize the analyzer inputs first (pub get, or the corpus generator for generated corpora)"
  printf '%s\n' "$base"
}

# One replay invocation: the daemon template substitutes {dir} with the
# corpus copy (the schema-freeze daemon shape: positional dir then --sdk);
# the batch template has no placeholder, so replay appends the copy as the
# last argument (the `analyze` positional dir).
run_replay() { # <pkg_dir> <edits_file> <replay_label> <out_dir>
  "$HARNESS_BIN" replay \
    --corpus-dir "$1" \
    --edits "$2" \
    --dartq-cmd "$DARTQ_BIN analyze --format=json --sdk $SDK" \
    --daemon-cmd "$DARTQ_BIN daemon {dir} --sdk $SDK" \
    --label "$3" \
    --out "$4"
}

# The gate's decision on one replay report: the file must exist and record
# a pass. Checked in addition to the replay exit code, so a harness that
# ever exited 0 without judging everything cannot pass silently — and so
# the negative control's verdict-flip exercises a real decision input.
verify_report() { # <report.json>
  [ -f "$1" ] || return 1
  jq -e '.verdict.passed == true' "$1" >/dev/null 2>&1
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/dq-incr-gate.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

if [ "$negative_control" -eq 1 ]; then
  # --- fault-injection self-check --------------------------------------------
  seq_dir="$1"
  seq_name="$(basename "$seq_dir")"
  out="$WORK/out"
  echo "== negative control over sequence '$seq_name'"

  corpus="$(resolve_corpus "$seq_dir" "$WORK/pkg")"
  if ! run_replay "$corpus" "$seq_dir/edits.jsonl" "$seq_name" "$out"; then
    fail "negative control: the baseline replay of '$seq_name' failed — the gate cannot self-check without a green baseline"
  fi
  if ! verify_report "$out/$seq_name/report.json"; then
    fail "negative control: the baseline report of '$seq_name' does not record a pass"
  fi
  echo "-- baseline green"

  # Injection 1: corrupt the expected output (flip the recorded verdict in
  # a copied report) — the same verdict check the gate loop applies must
  # reject it.
  jq '.verdict.passed = false' "$out/$seq_name/report.json" >"$WORK/flipped.json" ||
    die "flipping the control report failed"
  if verify_report "$WORK/flipped.json"; then
    fail "negative control: verdict-flip was NOT detected — verify_report accepted a failing report (the gate cannot fail)"
  fi
  echo "-- verdict-flip detected (verify_report rejected the corrupted report)"

  # Injection 2: corrupt the input (first step's line far out of range) —
  # replay must exit non-zero.
  {
    head -n 1 "$seq_dir/edits.jsonl" | jq -c '.line = 9999'
    tail -n +2 "$seq_dir/edits.jsonl"
  } >"$WORK/corrupted.jsonl"
  if run_replay "$WORK/pkg" "$WORK/corrupted.jsonl" "$seq_name-corrupt" "$out"; then
    fail "negative control: input corruption was NOT detected — replay accepted an out-of-range edit step (the gate cannot fail)"
  fi
  echo "-- input-corruption detected (replay rejected the out-of-range step)"

  # Injection 3: the vacuous signature — a genuinely clean package and a
  # no-op edit make the whole sequence compare zero diagnostics on BOTH
  # faces. The harness's sequence-level non-vacuity guard must fail the
  # run: all-equal on empty sets proves nothing, and a universe-narrowing
  # break that silenced both arms identically would otherwise pass here.
  # The probe package is real, so the guard — not an incidental error —
  # is what must trip: the run must exit non-zero AND its report must
  # carry the vacuity failure reason.
  mkdir -p "$WORK/vacuous/lib"
  cat >"$WORK/vacuous/pubspec.yaml" <<'EOF'
name: incr_vacuity_probe
environment:
  sdk: ^3.5.0
EOF
  printf 'void main() {}\n' >"$WORK/vacuous/lib/main.dart"
  printf '{"file":"lib/main.dart","line":1,"column":1,"length":0,"new_text":"// probe\\n"}\n' >"$WORK/vacuous/edits.jsonl"
  materialize_sequence "$WORK/vacuous" "$WORK/vacuous-pkg"
  if run_replay "$WORK/vacuous-pkg" "$WORK/vacuous/edits.jsonl" "$seq_name-vacuous" "$out"; then
    fail "negative control: vacuity was NOT detected — replay passed an all-equal sequence that compared zero diagnostics on both faces (the non-vacuity guard is broken)"
  fi
  if ! jq -e '.verdict.passed == false and (.verdict.failure_reason | test("vacuous"))' \
    "$out/$seq_name-vacuous/report.json" >/dev/null 2>&1; then
    fail "negative control: the vacuous run's report does not record the vacuity failure — replay exited non-zero for an unrelated cause (the guard itself did not trip)"
  fi
  echo "-- vacuity detected (the non-vacuity guard failed the zero-diagnostics all-equal run)"

  echo "== negative control PASSED: all three injected faults tripped the gate"
  exit 0
fi

# --- gate loop ----------------------------------------------------------------

evidence="$OUT_ROOT/$label"
[ -e "$evidence" ] &&
  die "$evidence already exists — refusing to overwrite recorded gate evidence (bump the label)"
mkdir -p "$evidence" || die "creating $evidence failed"

failed=""
for seq_dir in "$@"; do
  seq_name="$(basename "$seq_dir")"
  echo "== [$seq_name] replaying edit sequence"
  corpus="$(resolve_corpus "$seq_dir" "$WORK/$seq_name")"
  if ! run_replay "$corpus" "$seq_dir/edits.jsonl" "$seq_name" "$evidence"; then
    failed="$seq_name (replay exited non-zero; report: $evidence/$seq_name/report.json)"
    break
  fi
  if ! verify_report "$evidence/$seq_name/report.json"; then
    failed="$seq_name (report does not record a pass: $evidence/$seq_name/report.json)"
    break
  fi
  echo "-- [$seq_name] PASS"
done

if [ -n "$failed" ]; then
  fail "incremental consistency BROKEN in sequence: $failed"
fi

echo "incremental-gate: CHAIN PASSED (every sequence byte-equal per step)"
