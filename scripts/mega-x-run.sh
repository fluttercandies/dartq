#!/usr/bin/env bash
# mega-x-run.sh — the adversarial-axis gate over the generated mega-x corpus
# (#1811; the gating layer riding on the #1810 corpus-gen adversarial axes).
#
# mega-x is the SAME corpus-gen output as mega/ (pinned --seed 0) but with the
# zadv/ adversarial overlay asserted, not just present. The generated trees
# live under tests/corpus/mega-x/<scale>/ (untracked — regenerate via this
# script); the tracked golden slice at tests/corpus/mega-x/slice/ is the
# byte-stability pin corpus-gen's own tests cover (never regenerated here).
#
# Legs, in fixed order (the runner always runs all four):
#   X1  rss-100k @ 100k — the adversarial memory-shape gate: peak tree RSS of
#       one full dartq analysis over the regenerated 100k tree (zadv/ rides
#       inside it), normalized per 100k LOC; the harness's built-in 8 GiB
#       watchdog and, under --strict, the RSS charter bar (X1 row below).
#   X2  valid-subset differential @ 1k — `harness run` over a materialized
#       `1k-valid` copy of the 1k tree with the diagnostics-by-design files
#       removed: zadv/errstorm_* (the storm subtree — leg X3 owns it) and the
#       zadv/unicode variants index%8 >= 5 (doc-comment bidi markers ->
#       text_direction_code_point_in_comment, unicode URI literals ->
#       uri_does_not_exist, non-ASCII identifier edges -> illegal_character).
#       What remains is the ruled-clean regime, so the face runs
#       --allow-vacuous (mega-run oracle-leg precedent); under --strict the
#       ERROR (code,file) Jaccard still asserts >= bar (an empty-vs-empty
#       oracle only passes at 1.0 when dartq is also silent).
#   X3  storm inverse leg @ 1k — `harness run` over EVERY zadv/errstorm_NNNN
#       package WITHOUT --allow-vacuous: a vacuous oracle side means the
#       storm produced nothing for the oracle to see — the exact failure
#       this leg exists to catch (the harness dies exit 2 on vacuity and the
#       script scores it a leg breach, not an environment error). Under
#       --strict the ERROR Jaccard >= bar row asserts on top: dartq must
#       carry every storm diagnostic the oracle does.
#   X4  replay leg @ 1k — `harness replay` drives the generated edits.jsonl
#       (corpus-gen --emit-replay-seq, replay_edit.rs schema) over the 1k
#       tree: one live dartq daemon session (overlay didChange + the
#       dartq/analyze pull) vs a fresh full recompute per step, byte-equal
#       per step, non-vacuous across the sequence (harness exit 0 + the
#       report's verdict.passed, the incremental-gate check).
#
# --strict turns the run into the acceptance gate face (the strict
# acceptance ledger): per-leg rows are asserted and the run gets a labeled
# VERDICT. Asserted faces: X1 rss_per_100k_loc_kib <= 200000 KiB at the 100k
# tier (the RSS charter row, decimal reading — same bar as mega-run A1), the
# 8 GiB watchdog cap (X5), zero-panic + completion inside the driver timeout
# (X6/X7, the mega-run A10/A11 analogs), X2 ERROR Jaccard >= bar, X3 every
# storm package non-vacuous AND at bar, X4 replay pass + non-vacuous, and
# X8 corpus fingerprint echo + seed-0 identity on both trees. Without
# --strict the runner asserts nothing (warm-up/baseline shape; never
# exits 1).
#
# SERIAL PHASE ONLY: the run spawns real dartq processes and regenerates up
# to ~100k files (~400 MB on disk). Never run it alongside
# development-phase lanes (memory-incident discipline). serial-gates.sh
# invokes it as the mega-x-r1 step. --dry-run resolves every path, prints
# the plan and the exact commands, and executes nothing — no regeneration,
# no config write, no bench.
#
# Usage:
#   scripts/mega-x-run.sh [--strict] [--dry-run] [--label L] [--help]
#
#   --strict        acceptance gate face: assert the leg rows, print a
#                   VERDICT line, exit 1 on any breach.
#   --dry-run       print the plan + exact commands, execute nothing
#                   (--dry-run --strict prints the asserted table).
#   --label L       report directory name under target/evidence/mega-x/.
#                   Default: MEGAX_LABEL env, else mega-x-r1. A label whose
#                   directory already exists refuses the run (exit 2) —
#                   recorded evidence is never overwritten.
#
# Environment overrides: DARTQ_CMD (the dartq side; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`, the m1-gate shape — also the replay batch face and, with
# `daemon` swapped for `analyze`, the daemon face), DARTQ_BIN (replay leg's
# binary when DARTQ_CMD is overridden; default ./target/debug/dartq),
# FVM_HOME (default ~/fvm; the oracle dart for the differential legs),
# DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded to the harness drivers; default
# raised to 600000 here — a 100k-file batch analysis exceeds the harness's
# 120 s default). --strict bar overrides (non-negative integers, validated
# up front): MEGAX_RSS_100K_LIMIT_KIB (X1, KiB). Differential bar override
# (decimal in [0.0, 1.0]): MEGAX_ORACLE_JACCARD_MIN (X2/X3, default 1.0).
#
# Exit codes: 0 every leg produced its report — and, under --strict, every
# asserted row passed; 1 a --strict acceptance breach (watchdog kill,
# driver timeout, dartq crash, a Jaccard below bar, a vacuous storm leg, a
# replay divergence); 2 usage/environment error (missing binaries, jq
# missing, regeneration or materialization failure, missing or unreadable
# reports, invalid MEGAX_* bar, an existing report label).

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

OUT_ROOT="target/evidence/mega-x"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="mega-x-r1"
DEFAULT_TIMEOUT_MS=600000
# Replay sequence length generated into the 1k tree (corpus-gen
# --emit-replay-seq; also baked into the golden slice).
REPLAY_STEPS=30
# The unicode variants that emit diagnostics BY DESIGN (index % 8 >= 5):
# 5 = doc-comment bidi markers, 6 = unicode URI literal, 7 = non-ASCII
# identifier edge. They leave the X2 clean face; X3 storms the other
# diagnostics-by-design subtree.
UNICODE_DIAG_MOD=8
UNICODE_DIAG_MIN=5

# --- strict acceptance bars (asserted only under --strict) --------------------
# X1: the RSS charter row — "RSS/100k LOC <= 200 MB" decimal reading,
# 200000 KiB, same bar as mega-run A1. Asserted at the 100k tier.
# Env override: MEGAX_RSS_100K_LIMIT_KIB.
RSS_100K_LIMIT_KIB=200000
# X5: tree-RSS watchdog cap the harness itself enforces (runbook:8;
# tests/harness/src/bench.rs WATCHDOG_THRESHOLD_KIB). A breach kills the
# driver group and fails the bench; strict mode classifies it as a FAIL.
WATCHDOG_CAP_KIB=8388608
# X2/X3: ERROR (code,file) Jaccard bar — full parity like every differential
# gate (the full-parity bars ADR). The harness enforces it as its own
# --threshold; this script classifies the harness exit. Env override:
# MEGAX_ORACLE_JACCARD_MIN (decimal in [0,1]).
ORACLE_JACCARD_MIN=1.0
# Oracle-of-record caliber (mirrors mega-run.sh; a mismatch is a WARN, not
# fatal — the report meta records the real banner either way).
EXPECTED_ORACLE_VERSION="3.13.1"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "mega-x-run: $*" >&2
  exit 2
}

dry_run=0
strict=0
label="${MEGAX_LABEL:-$DEFAULT_LABEL}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --strict) strict=1 ;;
  --label)
    [ $# -ge 2 ] || die "--label needs a value"
    label="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/mega-x-run.sh --help)"
    ;;
  esac
  shift
done

oracle_jac_min="${MEGAX_ORACLE_JACCARD_MIN:-$ORACLE_JACCARD_MIN}"
case "$oracle_jac_min" in
'' | *[!0-9.]*) die "MEGAX_ORACLE_JACCARD_MIN must be a decimal in [0.0, 1.0] (got '$oracle_jac_min')" ;;
esac
if ! awk -v v="$oracle_jac_min" 'BEGIN { exit !(v >= 0.0 && v <= 1.0) }'; then
  die "MEGAX_ORACLE_JACCARD_MIN must be in [0.0, 1.0] (got '$oracle_jac_min')"
fi

if [ "$strict" -eq 1 ]; then
  rss_limit_kib="${MEGAX_RSS_100K_LIMIT_KIB:-$RSS_100K_LIMIT_KIB}"
  case "$rss_limit_kib" in
  '' | *[!0-9]*) die "MEGAX_RSS_100K_LIMIT_KIB must be a non-negative integer (got '$rss_limit_kib')" ;;
  esac
fi

# --- preflight ---------------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  die "jq is required for config materialization and the summary (brew install jq)"
fi
if [ ! -x "$HARNESS" ]; then
  die "$HARNESS not found — build it first: cargo build -p harness"
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
# Pin the oracle binary to its physical path (mega-run.sh idiom: the fvm
# `default` component is a symlink).
if dart_dir="$(cd "$(dirname "$dart_bin")" 2>/dev/null && pwd -P)"; then
  dart_bin="$dart_dir/dart"
fi
DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
DARTQ_BIN="${DARTQ_BIN:-./target/debug/dartq}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "$DARTQ_BIN" ]; then
  die "$DARTQ_BIN not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)"
fi
if [ ! -x "$dart_bin" ]; then
  die "oracle dart not found at $dart_bin (FVM_HOME=$FVM_HOME) — install the fvm default pin or point FVM_HOME at it"
fi

# Oracle-crash bracket guard: a tree whose patho mega-expression exceeds the
# verified-clean bracket pre-dates the generator's term cap (#2148) and
# crashes `dart analyze` (rc 255, "Internal error: Stack Overflow") wherever
# the file enters the analyzed set — subset materialization cannot make such
# a tree self-consistent, the stale fixture itself is the fault. The pinned
# generator (tools/corpus-gen, PATHO_EXPR_TOKENS) caps the left-nested `+`
# chain at 2,000 terms (>=4,000 overflows the analyzer's unguarded recursive
# AST walks; at ~20k terms the overflow surfaces in summary2 informative
# data). Fresh trees emit at most 1,999 `+` per patho file and every other
# shape emits a handful, so >=4,000 in one file is the deterministic
# stale-generator signature.
scale_is_stale() {
  dir="tests/corpus/mega-x/$1"
  for f in "$dir"/zadv/patho/patho_*.dart; do
    [ -f "$f" ] || return 1
    if [ "$(tr -cd '+' < "$f" | wc -c)" -ge 4000 ]; then
      return 0
    fi
  done
  return 1
}

need_regen=""
for s in 1k 100k; do
  if [ ! -f "tests/corpus/mega-x/$s/manifest.json" ] || scale_is_stale "$s"; then
    need_regen="$need_regen $s"
  fi
done
if [ -n "$need_regen" ] && [ "$dry_run" -eq 0 ] && ! command -v cargo >/dev/null 2>&1; then
  die "cargo is required to regenerate missing or stale scales:$need_regen"
fi

# A 100k-file batch analysis exceeds the harness's 120 s default budget.
if [ -z "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS:-}" ]; then
  export DARTQ_HARNESS_ORACLE_TIMEOUT_MS="$DEFAULT_TIMEOUT_MS"
fi

# --- helpers (run path only; --dry-run never calls these) --------------------

# Regenerate one mega-x scale with the pinned seed if its tree is absent or
# stale (the oracle-crash bracket guard above). The 1k tree additionally
# carries the replay sequence (axis 2a) for X4.
regen_scale() {
  s="$1"
  dir="tests/corpus/mega-x/$s"
  if [ -f "$dir/manifest.json" ] && ! scale_is_stale "$s"; then
    return 0
  fi
  echo "-- [$s] tree absent or stale; regenerating (corpus-gen --seed 0)"
  if [ "$s" = "1k" ]; then
    cargo run -q -p corpus-gen -- --out "$dir" --scale "$s" --seed 0 \
      --emit-replay-seq "$REPLAY_STEPS" || return 1
  else
    cargo run -q -p corpus-gen -- --out "$dir" --scale "$s" --seed 0 || return 1
  fi
}

# Materialize the hand-written package_config.json the generated tree lacks
# (mega-run.sh idiom): the app package plus every packages/* entry,
# config-file-relative rootUri, languageVersion 3.13 (the harness corpus
# preflight's floor). Idempotent.
materialize_config() {
  s="$1"
  dir="tests/corpus/mega-x/$s"
  config="$dir/.dart_tool/package_config.json"
  if [ -f "$config" ]; then
    return 0
  fi
  echo "-- [$s] materializing $config"
  mkdir -p "$dir/.dart_tool" || return 1
  entries='[{"name":"app","rootUri":"..","packageUri":"lib/","languageVersion":"3.13"}]'
  for d in "$dir"/packages/*/; do
    [ -f "${d}pubspec.yaml" ] || continue
    name="$(sed -n 's/^name: *//p' "${d}pubspec.yaml" | head -n1)"
    [ -n "$name" ] || continue
    rel="../${d#"$dir"/}"
    rel="${rel%/}"
    entries="$(printf '%s' "$entries" | jq -c --arg n "$name" --arg r "$rel" \
      '. + [{name: $n, rootUri: $r, packageUri: "lib/", languageVersion: "3.13"}]')" || return 1
  done
  jq -n --argjson p "$entries" '{configVersion: 2, packages: $p}' >"$config" || return 1
}

# Materialize the X2 corpus: a copy of the 1k tree minus the
# diagnostics-by-design files — zadv/errstorm_* (X3 owns the storm subtree)
# and the zadv/unicode variants index%8 >= 5. The materialized package
# config copies over verbatim: its rootUri entries are config-relative, so
# the same layout at a sibling path resolves identically.
materialize_valid_subset() {
  src="tests/corpus/mega-x/1k"
  dst="$outdir/1k-valid"
  echo "-- [1k] materializing the valid subset: $dst (errstorm + unicode diag variants excluded)"
  rm -rf "$dst" || return 1
  mkdir -p "$dst" || return 1
  cp -R "$src/." "$dst/" || return 1
  rm -rf "$dst"/zadv/errstorm_* || return 1
  for f in "$dst"/zadv/unicode/uni_*.dart; do
    [ -f "$f" ] || continue
    n="$(basename "$f")"
    n="${n#uni_}"
    n="${n%.dart}"
    n="$((10#$n))"
    if [ "$((n % UNICODE_DIAG_MOD))" -ge "$UNICODE_DIAG_MIN" ]; then
      rm -f "$f" || return 1
    fi
  done
}

# Materialize the package_config.json a standalone storm package needs for
# the corpus preflight (its own package entry, languageVersion 3.13).
materialize_storm_config() {
  pkg="$1"
  config="$pkg/.dart_tool/package_config.json"
  if [ -f "$config" ]; then
    return 0
  fi
  name="$(sed -n 's/^name: *//p' "$pkg/pubspec.yaml" | head -n1)"
  [ -n "$name" ] || return 1
  mkdir -p "$pkg/.dart_tool" || return 1
  jq -n --arg n "$name" \
    '{configVersion: 2, packages: [{name: $n, rootUri: "../", packageUri: "lib/", languageVersion: "3.13"}]}' \
    >"$config" || return 1
}

# Classify one failed bench invocation for the strict gate (mega-run.sh
# idiom): watchdog kill (X5), wall-clock timeout (X7), dartq exit outside
# the valid 0-3 analysis tiers (X6). Sets $failure_note and returns 0 for a
# breach; returns 1 for harness-own.
classify_bench_failure() {
  if grep -q "RSS watchdog" "$errfile"; then
    failure_note="watchdog killed the driver tree at the ${WATCHDOG_CAP_KIB} KiB cap (X5)"
  elif grep -q "timed out after" "$errfile"; then
    failure_note="driver exceeded the ${DARTQ_HARNESS_ORACLE_TIMEOUT_MS} ms budget and was killed (X7)"
  elif grep -q "exited with" "$errfile"; then
    failure_note="dartq exited outside the valid 0-3 analysis tiers — crash or panic (X6)"
  else
    return 1
  fi
}

# Classify one failed `harness run` invocation for a differential face.
# Exit 0 = the Jaccard met the bar; exit 1 = a measured below-bar result;
# exit 2 carrying the vacuity marker on a storm face = the storm produced no
# oracle entries (the X3 inverse assert's failure shape — a leg breach, not
# an environment error); any other exit 2 is harness-own. Sets
# $face_class and returns 0 for a leg that ran, 1 for harness-own.
classify_run_failure() {
  face="$1"
  if grep -q "vacuous" "$errfile"; then
    if [ "$face" = "storm" ]; then
      failure_note="storm package produced a VACUOUS oracle side — the recovery storms never reached the oracle (X3 inverse assert)"
      return 0
    fi
    return 1
  fi
  failure_note="harness error (see its stderr)"
  return 1
}

# Run one `harness run` face. $1 = face name ("valid" | "storm"), $2 =
# corpus dir; extra flags (e.g. --allow-vacuous) after $2. A successful or
# below-bar/vacuous-storm run appends breaches under --strict; a
# harness-own failure dies as an environment error with no verdict scored.
run_differential() {
  face="$1"
  dir="$2"
  shift 2
  odir="$outdir/diff-$face"
  if "$HARNESS" run --corpus-dir "$dir" --oracle-dart "$dart_bin" --dartq-cmd "$DARTQ_CMD" \
    --severity-filter ERROR --threshold "$oracle_jac_min" --out "$odir" "$@" 2>"$errfile"; then
    cat "$errfile" >&2
    return 0
  fi
  rc=$?
  cat "$errfile" >&2
  if [ "$rc" -eq 1 ]; then
    failure_note="ERROR (code,file) Jaccard below the $oracle_jac_min bar"
  elif [ "$rc" -eq 2 ]; then
    # A storm face's vacuous oracle side is the leg breach this runner
    # exists to catch; anything else exit-2 is harness-own.
    if ! classify_run_failure "$face"; then
      die "harness-own failure during the diff-$face leg (no verdict scored)"
    fi
  else
    die "harness-own failure during the diff-$face leg (rc=$rc, no verdict scored)"
  fi
  strict_fail=1
  breaches="${breaches}[$label] diff-$face ($dir): $failure_note
"
  return 1
}

# One differential summary row: the ERROR Jaccard + entry counts off the
# harness report (same jq shape as mega-run's oracle_row). $1 = ledger row
# name, $2 = report.json path, $3 = "ok"|"fail" (only matters under
# --strict).
differential_row() {
  row_name="$1"
  rj="$2"
  state="$3"
  row="$(jq -r '[(.error_jaccard * 1000 | round / 1000 | tostring),
                 (.oracle_total | tostring),
                 (.dartq_total | tostring),
                 (.oracle_only | length | tostring),
                 (.dartq_only | length | tostring)] | @tsv' "$rj" 2>/dev/null)" || row=""
  if [ -z "$row" ]; then
    printf '  %-16s no report at %s — leg incomplete\n' "$row_name" "$rj"
    return 1
  fi
  IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF
  verdict_word="record"
  if [ "$strict" -eq 1 ]; then
    if [ "$state" = "ok" ]; then verdict_word="PASS"; else verdict_word="FAIL"; fi
  fi
  printf '  %-16s jaccard %s | oracle %s vs dartq %s diags (%s/%s diff entries) — %s\n' \
    "$row_name" "$jac" "$ototal" "$dtotal" "$oonly" "$donly" "$verdict_word"
}

# --- plan --------------------------------------------------------------------
# Refuse-overwrite guard (mega-run.sh/m2-gate precedent): every entry path
# writes evidence under the same label root, so one check covers them all.
if [ -e "$OUT_ROOT/$label" ]; then
  die "$OUT_ROOT/$label already exists — refusing to overwrite recorded gate evidence (pass a new --label)"
fi
echo "== mega-x-run plan"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s\n' "dartq bin" "$DARTQ_BIN"
printf '  %-16s %s (oracle-of-record)\n' "oracle dart" "$dart_bin"
printf '  %-16s %s\n' "trees" "tests/corpus/mega-x/{1k,100k} (corpus-gen --seed 0; 1k carries --emit-replay-seq $REPLAY_STEPS)"
for s in 1k 100k; do
  dir="tests/corpus/mega-x/$s"
  if [ -f "$dir/manifest.json" ]; then
    state="present"
  else
    state="ABSENT -> will regenerate"
  fi
  printf '  %-16s %s (%s)\n' "scale $s" "$dir" "$state"
done
printf '  %-16s %s\n' "legs" "X1 rss-100k@100k -> X2 valid-subset diff@1k -> X3 storm diff@1k -> X4 replay@1k"
printf '  %-16s target/evidence/mega-x/%s/\n' "reports" "$label"
if [ "$strict" -eq 1 ]; then
  echo "== strict acceptance gate"
  printf '  %-6s %s\n' "X1" "rss_per_100k_loc_kib <= $rss_limit_kib KiB at 100k (the RSS charter row, decimal; adversarial overlay riding)"
  printf '  %-6s %s\n' "X2" "ERROR (code,file) Jaccard >= $oracle_jac_min on the 1k valid subset (errstorm + unicode diag variants excluded; --allow-vacuous, ruled-clean regime)"
  printf '  %-6s %s\n' "X3" "every zadv/errstorm_* package: oracle side non-vacuous (vacuity = leg breach) AND ERROR Jaccard >= $oracle_jac_min"
  printf '  %-6s %s\n' "X4" "harness replay over the generated edits.jsonl: every step byte-equal, sequence non-vacuous, verdict.passed"
  printf '  %-6s %s\n' "X5" "watchdog tree cap $WATCHDOG_CAP_KIB KiB — harness breach = FAIL"
  printf '  %-6s %s\n' "X6" "zero panic — dartq exit outside the valid 0-3 tiers = FAIL"
  printf '  %-6s %s\n' "X7" "completion — driver timeout at ${DARTQ_HARNESS_ORACLE_TIMEOUT_MS} ms = FAIL"
  printf '  %-6s %s\n' "X8" "manifest fingerprint echoed + seed-0 corpus identity on both trees"
  printf '  %-6s %s\n' "env" "bar overrides: MEGAX_RSS_100K_LIMIT_KIB MEGAX_ORACLE_JACCARD_MIN"
fi
if [ "$dry_run" = 1 ]; then
  echo "== dry run: plan resolved, nothing executed"
  exit 0
fi

# --- run: strict verdict accumulators (every leg's report stays independent;
# the final verdict aggregates every breach) -----------------------------------
strict_fail=0
breaches=""
errfile="$(mktemp "${TMPDIR:-/tmp}/mega-x-run.XXXXXX")" || die "mktemp failed"
trap 'rm -f "$errfile"' EXIT
outdir="$OUT_ROOT/$label"

# Oracle-of-record banner check (mega-run.sh idiom — the report meta records
# the real version either way, a mismatch is a WARN).
found_ver="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
if [ "$found_ver" != "$EXPECTED_ORACLE_VERSION" ]; then
  echo "mega-x-run: WARN oracle dart expected $EXPECTED_ORACLE_VERSION, found '${found_ver:-unrecognized banner}'"
fi

# --- corpus identity (X8): both trees must come from the pinned generator
# contract or no measurement is valid — environment-tier failure, before any
# leg runs.
for s in 1k 100k; do
  regen_scale "$s" || die "regeneration of mega-x/$s failed"
  materialize_config "$s" || die "config materialization for mega-x/$s failed"
  fp="$(jq -r '.fingerprint // ""' "tests/corpus/mega-x/$s/manifest.json" 2>/dev/null)" || fp=""
  seed="$(jq -r '.seed // ""' "tests/corpus/mega-x/$s/manifest.json" 2>/dev/null)" || seed=""
  if [ -z "$fp" ] || [ "$seed" != "0" ]; then
    die "corpus $s identity check failed (X8): fingerprint='$fp' seed='$seed' — regenerate with corpus-gen --seed 0"
  fi
  printf '  %-16s %s (seed %s) — corpus identity echoed (X8)\n' "corpus_id $s" "$fp" "$seed"
done

# --- X1: adversarial RSS @ 100k ------------------------------------------------
echo
echo "== [$label] X1 rss-100k @ mega-x/100k (adversarial overlay riding)"
x1_ok=1
if "$HARNESS" bench --metric rss-100k --corpus-dir "tests/corpus/mega-x/100k" --out "$outdir" \
  --dartq-cmd "$DARTQ_CMD" 2>"$errfile"; then
  cat "$errfile" >&2
else
  cat "$errfile" >&2
  classify_bench_failure || die "harness-own failure during X1 (no verdict scored)"
  x1_ok=0
  strict_fail=1
  breaches="${breaches}[$label] X1 rss-100k: $failure_note
"
fi

# --- X2: valid-subset differential @ 1k ----------------------------------------
echo
echo "== [$label] X2 valid-subset differential @ mega-x/1k (ruled-clean regime)"
materialize_valid_subset || die "valid-subset materialization failed"
x2_ok=1
run_differential valid "$outdir/1k-valid" --allow-vacuous || x2_ok=0

# --- X3: storm inverse leg @ 1k ------------------------------------------------
echo
echo "== [$label] X3 storm inverse leg @ mega-x/1k/zadv/errstorm_*"
x3_ok=1
x3_count=0
for pkg in tests/corpus/mega-x/1k/zadv/errstorm_*/; do
  [ -f "${pkg}pubspec.yaml" ] || continue
  x3_count=$((x3_count + 1))
  materialize_storm_config "${pkg%/}" || die "storm config materialization failed for $pkg"
  run_differential "storm-${x3_count}" "${pkg%/}" || x3_ok=0
done
if [ "$x3_count" -eq 0 ]; then
  die "no storm packages under tests/corpus/mega-x/1k/zadv — corpus missing the overlay"
fi

# --- X4: replay leg @ 1k -------------------------------------------------------
echo
echo "== [$label] X4 replay leg @ mega-x/1k (generated edits.jsonl, $REPLAY_STEPS steps)"
x4_ok=1
edits="tests/corpus/mega-x/1k/edits.jsonl"
[ -f "$edits" ] || die "$edits missing — the 1k tree was generated without --emit-replay-seq; delete it and rerun"
if "$HARNESS" replay \
  --corpus-dir "tests/corpus/mega-x/1k" \
  --edits "$edits" \
  --dartq-cmd "$DARTQ_BIN analyze --format=machine --sdk $sdk_root" \
  --daemon-cmd "$DARTQ_BIN daemon {dir} --sdk $sdk_root" \
  --label x4-replay --out "$outdir" 2>"$errfile"; then
  cat "$errfile" >&2
  # The incremental-gate double check: exit 0 AND the recorded verdict.
  if ! jq -e '.verdict.passed == true' "$outdir/x4-replay/report.json" >/dev/null 2>&1; then
    x4_ok=0
    strict_fail=1
    breaches="${breaches}[$label] X4 replay: exit 0 but report.json does not record verdict.passed ($outdir/x4-replay/report.json)
"
  fi
else
  rc=$?
  cat "$errfile" >&2
  x4_ok=0
  if [ "$rc" -eq 1 ]; then
    failure_note="incremental divergence or a vacuous all-equal sequence (the report names which)"
    strict_fail=1
    breaches="${breaches}[$label] X4 replay: $failure_note
"
  else
    die "harness-own failure during X4 (rc=$rc, no verdict scored)"
  fi
fi

# --- summary -------------------------------------------------------------------
echo
echo "== [$label] summary"
if [ "$x1_ok" -eq 1 ]; then
  rss="$(jq -r '[.file_count, .loc, .peak_rss_kib, .rss_per_100k_loc_kib] | @tsv' \
    "$outdir/rss-100k/100k/report.json" 2>/dev/null)" || rss=""
  [ -n "$rss" ] || die "report $outdir/rss-100k/100k/report.json is missing or unreadable"
  IFS="$(printf '\t')" read -r files loc peak_rss per_100k <<EOF
$rss
EOF
  printf '  %-16s %s files, %s LOC\n' "corpus 100k" "$files" "$loc"
  printf '  %-16s %s KiB peak = %s KiB per 100k LOC\n' "rss_100k" "$peak_rss" "$per_100k"
  if [ "$strict" -eq 1 ]; then
    a1="$(jq -r '.rss_per_100k_loc_kib | ceil' "$outdir/rss-100k/100k/report.json")"
    if [ "$a1" -le "$rss_limit_kib" ]; then
      printf '  %-16s %s KiB <= %s KiB (the RSS charter row, decimal) — PASS\n' "X1 rss/100k" "$a1" "$rss_limit_kib"
    else
      printf '  %-16s %s KiB > %s KiB (the RSS charter row, decimal) — FAIL\n' "X1 rss/100k" "$a1" "$rss_limit_kib"
      strict_fail=1
      breaches="${breaches}[$label] X1: rss_per_100k_loc_kib $a1 > $rss_limit_kib KiB
"
    fi
    printf '  %-16s peak %s KiB within the %s KiB cap, watchdog armed — PASS\n' \
      "X5 watchdog" "$peak_rss" "$WATCHDOG_CAP_KIB"
    printf '  %-16s dartq exits within the valid 0-3 tiers — PASS\n' "X6 zero-panic"
    printf '  %-16s inside the %s ms driver budget — PASS\n' "X7 completion" "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS}"
  fi
elif [ "$strict" -eq 1 ]; then
  printf '  %-16s FAILED — breach recorded, see the final verdict\n' "X1 rss/100k"
fi

if [ "$x2_ok" -eq 1 ]; then
  differential_row "X2 valid" "$outdir/diff-valid/report.json" ok
elif [ "$strict" -eq 1 ]; then
  differential_row "X2 valid" "$outdir/diff-valid/report.json" fail || true
fi

for pkg_report in "$outdir"/diff-storm-*/report.json; do
  [ -f "$pkg_report" ] || continue
  storm_name="$(basename "$(dirname "$pkg_report")")"
  differential_row "X3 ${storm_name#diff-}" "$pkg_report" "$([ "$x3_ok" -eq 1 ] && printf ok || printf fail)" || true
done
if [ "$x3_ok" -eq 0 ] && [ "$strict" -eq 1 ]; then
  printf '  %-16s FAILED — breach recorded, see the final verdict\n' "X3 storms"
fi
printf '  %-16s %s storm package(s) ran the inverse leg\n' "X3 count" "$x3_count"

if [ "$x4_ok" -eq 1 ]; then
  x4_row="$(jq -r '[.verdict.equal_steps, .verdict.total_steps, .verdict.total_diagnostics_compared] | @tsv' \
    "$outdir/x4-replay/report.json" 2>/dev/null)" || x4_row=""
  IFS="$(printf '\t')" read -r eq_steps tot_steps tot_diags <<EOF
$x4_row
EOF
  printf '  %-16s %s/%s steps byte-equal, %s diagnostics compared — %s\n' \
    "X4 replay" "${eq_steps:-?}" "${tot_steps:-?}" "${tot_diags:-?}" \
    "$([ "$strict" -eq 1 ] && printf PASS || printf record)"
elif [ "$strict" -eq 1 ]; then
  printf '  %-16s FAILED — breach recorded, see the final verdict\n' "X4 replay"
fi

echo "  reports: $outdir/{rss-100k/100k,diff-valid,diff-storm-*,x4-replay}/report.json"

echo
if [ "$strict" -eq 1 ]; then
  if [ "$strict_fail" -eq 1 ]; then
    echo "mega-x-run: VERDICT FAIL (strict acceptance gate breached — label $label)"
    printf '%s\n' "$breaches" | while IFS= read -r b; do
      if [ -n "$b" ]; then printf '  %s\n' "$b"; fi
    done
    exit 1
  fi
  echo "mega-x-run: VERDICT PASS (strict acceptance gate met — label $label)"
  exit 0
fi
echo "mega-x-run: VERDICT COMPLETE (no thresholds asserted; exit 1 reserved for the acceptance run)"
exit 0
