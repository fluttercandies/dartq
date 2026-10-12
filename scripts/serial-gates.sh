#!/usr/bin/env bash
# serial-gates.sh — the serial verification window orchestrator (goal #927).
#
# Runs the whole gate marathon as one turnkey, strictly serialized chain:
# never two product processes at a time, every step under an RSS watchdog,
# every step logged, first red step stops the chain. It orchestrates the
# existing gate scripts (m1/m2/m3/lint/mega/acceptance/repro/coverage) and
# composes the two trend differential runs in the m0-gate.sh idiom (no
# separate trend script exists). It NEVER executes product code outside the
# watchdogged steps below. This script does not commit; repro-gate's
# clean-tracked-tree rule must be satisfied before the `repro` step (commit
# the tracked waves first — serial-phase-runbook Phase 4).
#
# The chain, in fixed order:
#   release-rebuild  cargo build --release -p dartq-cli -p harness
#                    (binary `dartq` per crates/dartq-cli [[bin]] ->
#                    ./target/release/dartq; the trend + acceptance steps
#                    drive the release face, hence -p harness too)
#   debug-rebuild    cargo build -p dartq-cli -p harness
#                    (m1/m2/m3/lint gates + mega-run drive
#                    ./target/debug/{dartq,harness})
#   trend-w5-A       run A: syntax universe (tests/corpus/syntax-codes.txt,
#                    255 codes, ERROR, bar 1.0) over the check face
#                    (`dartq check --format=machine`) on the trend corpora
#                    tests/corpus/m0 + tests/corpus/m0-syntax — the exact
#                    m0-gate.sh run-A shape, label trend-w5
#   trend-w5-B       run B: union universe (255 + 36 - 1 overlap = 290,
#                    composed and pinned exactly like m0-gate.sh) over the
#                    analyze face (`dartq analyze --format=machine --sdk
#                    <fvm default dart-sdk>`), label trend-w5
#   m1-r5            scripts/m1-gate.sh --label m1-r5 (ERROR, bar 1.0)
#   acceptance       scripts/acceptance-gate.sh --label serial
#                    (zero residual rows expected; a known-open defect may
#                    leave a single row — this script never suppresses rows,
#                    a red acceptance step stops the chain for adjudication)
#   lint-gate        scripts/lint-gate.sh --label lints-r1 (fresh label; the
#                    lint code-table freeze cross-checks run inside it)
#   m2-r1            scripts/m2-gate.sh --label m2-r1 (WARNING, bar 1.0)
#   m3-r1            M3_LABEL=... scripts/m3-gate.sh (env-only label)
#   mega-r1          perf + parity: scripts/mega-run.sh --scale 1k
#                    --oracle-leg --strict (parity leg, 1k-only by contract)
#                    then --scale ${SERIAL_MEGA_SCALE:-10k} --strict (perf
#                    face; "all" runs the full ladder incl. 100k)
#   scanner-suite    cargo test -p dartq-scanner  -- --test-threads 2
#   flow-suite       cargo test -p dartq-flow     -- --test-threads 2
#   parser-suite     cargo test -p dartq-parser   -- --test-threads 2
#   semantic-suite   cargo test -p dartq-semantic -- --test-threads 2
#   lints-suite      cargo test -p dartq-lints    -- --test-threads 2
#   engine-suite     cargo test -p dartq-engine   -- --test-threads 2
#   repro            scripts/repro-gate.sh (clean tracked tree required)
#   coverage         scripts/coverage-gate.sh --stability — the §19.5
#                    two-run protocol (identical run-to-run totals); LAST,
#                    never parallel with anything
#
# Watchdog (vendored from the /tmp/rss_cmd.sh pattern — /tmp is ephemeral,
# so this script is self-contained): each step's stdout+stderr goes to
# /tmp/serial_<step>.log with a trailing `exit=<code>` line; a 5 s poll sums
# the step's process-group RSS and, on breach, kills the step's whole
# process tree (descendant walk, not a group kill — the orchestrator shares
# the group and must survive) and appends
# `WATCHDOG_KILL rss=<KB> limit=<KB>`. Unlike the /tmp original, a watchdog
# kill propagates: the step is judged KILLED and the chain stops. Caps: 8 GiB
# everywhere (RSS_LIMIT_KB overrides), 12 GiB for mega-r1 (runbook cap above
# the bench's internal 8 GiB).
#
# Labels: every evidence-producing step gets a fresh default label; pass
# SERIAL_TAG=<suffix> to namespace a rerun (e.g. SERIAL_TAG=r2 ->
# trend-w5-r2, m1-r5-r2, mega-r1-r2). A gate whose evidence directory
# already exists refuses to run (exit 2) — recorded evidence is never
# overwritten; bump SERIAL_TAG or clean up consciously.
#
# Environment: SERIAL_TAG (label suffix), SERIAL_MEGA_SCALE (1k|10k|100k|all,
# default 10k), SERIAL_MEGA_ORACLE_LEG (1 default, 0 drops the 1k parity
# leg), SERIAL_COVERAGE_HTML (1 adds an html pass after the coverage
# stability pair), RSS_LIMIT_KB (default 8388608), FVM_HOME, DARTQ_CMD /
# DARTQ_ANALYZE_CMD (trend faces; defaults are the m0-gate shapes). The
# wrapped gate scripts honor their own env overrides unchanged.
#
# Usage:
#   scripts/serial-gates.sh [--from <step>] [--only <step>[,<step>...]]
#                           [--skip <step>...] [--help]
#
#   --from STEP     start the chain at STEP (everything before is skipped).
#   --only LIST     run exactly the named steps, in chain order (a red step
#                   can be re-run alone after a fix). Mutually exclusive
#                   with --from.
#   --skip LIST     drop the named steps from the selection (combinable with
#                   --from; with --only it filters the only-list).
#   Fail-fast: first failing step aborts the chain; the end summary always
#   prints step | status | wall | peak-RSS | log. Exit codes: 0 all selected
#   steps green; 1 a step failed or was watchdog-killed; 2 usage or
#   preflight error (missing jq/oracle/corpus .dart_tool, unknown step,
#   existing evidence directory).

set -Eeuo pipefail
cd "$(dirname "$0")/.." || exit 2

RSS_DEFAULT_KB="${RSS_LIMIT_KB:-8388608}"
MEGA_CAP_KB=12582912
M0_OUT_ROOT="docs/audits/gate-m0"
UNION_CODES="target/gate/union-codes.txt"
EXPECTED_UNION_COUNT=290
FVM_HOME="${FVM_HOME:-$HOME/fvm}"
SDK_ROOT="$FVM_HOME/default/bin/cache/dart-sdk"
DART_BIN="$SDK_ROOT/bin/dart"
CHECK_CMD="${DARTQ_CMD:-./target/release/dartq check --format=machine}"
ANALYZE_CMD="${DARTQ_ANALYZE_CMD:-./target/release/dartq analyze --format=machine --sdk $SDK_ROOT}"

TAG="${SERIAL_TAG:+-${SERIAL_TAG}}"
LBL_TREND="trend-w5${TAG}"
LBL_M1="m1-r5${TAG}"
LBL_ACC="serial${TAG}"
LBL_LINT="lints-r1${TAG}"
LBL_M2="m2-r1${TAG}"
LBL_M3="m3-r1${TAG}"
LBL_MEGA="mega-r1${TAG}"
LBL_REPRO="serial${TAG}"

STEPS=(
  release-rebuild debug-rebuild trend-w5-A trend-w5-B m1-r5 acceptance
  lint-gate m2-r1 m3-r1 mega-r1 scanner-suite flow-suite parser-suite
  semantic-suite lints-suite engine-suite repro coverage
)

usage() {
  sed -n '2,/^set -Eeuo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() { # <message...>  usage/preflight error
  echo "serial-gates: $*" >&2
  exit 2
}

is_step() { # <name>
  local s
  for s in "${STEPS[@]}"; do [ "$s" = "$1" ] && return 0; done
  return 1
}

in_list() { # <comma-list> <name>
  local item IFS=','
  for item in $1; do [ "$item" = "$2" ] && return 0; done
  return 1
}

index_of() { # <step> -> prints its chain index
  local i
  for i in "${!STEPS[@]}"; do
    [ "${STEPS[$i]}" = "$1" ] && {
      echo "$i"
      return 0
    }
  done
  return 1
}

cap_for_step() {
  case "$1" in
  mega-r1) echo "$MEGA_CAP_KB" ;;
  *) echo "$RSS_DEFAULT_KB" ;;
  esac
}

# --- argument parsing ---------------------------------------------------------

FROM_STEP=""
ONLY_LIST=""
SKIP_LIST=""
while [ $# -gt 0 ]; do
  case "$1" in
  --from)
    [ $# -ge 2 ] || die "--from needs a step name"
    FROM_STEP="$2"
    shift 2
    ;;
  --only)
    [ $# -ge 2 ] || die "--only needs a comma-separated step list"
    ONLY_LIST="$2"
    shift 2
    ;;
  --skip)
    [ $# -ge 2 ] || die "--skip needs a comma-separated step list"
    SKIP_LIST="$2"
    shift 2
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/serial-gates.sh --help)"
    ;;
  esac
done

[ -z "$FROM_STEP" ] || [ -z "$ONLY_LIST" ] || die "--from and --only are mutually exclusive"
[ -z "$FROM_STEP" ] || is_step "$FROM_STEP" || die "unknown --from step '$FROM_STEP'"
if [ -n "$ONLY_LIST" ]; then
  item="$(printf '%s' "$ONLY_LIST" | tr ',' ' ')"
  for item in $item; do is_step "$item" || die "unknown --only step '$item'"; done
fi
if [ -n "$SKIP_LIST" ]; then
  item="$(printf '%s' "$SKIP_LIST" | tr ',' ' ')"
  for item in $item; do is_step "$item" || die "unknown --skip step '$item'"; done
fi

selected() { # <step> -> 0 if the step runs in this invocation
  if [ -n "$ONLY_LIST" ]; then
    in_list "$ONLY_LIST" "$1" || return 1
  elif [ -n "$FROM_STEP" ]; then
    local seen=0 s
    for s in "${STEPS[@]}"; do
      [ "$s" = "$FROM_STEP" ] && seen=1
      [ "$s" = "$1" ] && [ "$seen" = 1 ] && return 0
    done
    return 1
  fi
  ! in_list "$SKIP_LIST" "$1"
}

# --- watchdog (vendored; self-contained, no /tmp dependency) ------------------

WATCH_PEAK_KB=0
WATCH_LOG=""

kill_tree() { # <pid> — kill the step's whole descendant tree, not its group
  local child
  for child in $(pgrep -P "$1" 2>/dev/null); do kill_tree "$child"; done
  kill -9 "$1" 2>/dev/null || true
}

watchdog_run() { # <name> <rss_limit_kb> <cmd...> — sets WATCH_PEAK_KB, WATCH_LOG
  local name="$1" limit="$2"
  shift 2
  WATCH_LOG="/tmp/serial_${name}.log"
  WATCH_PEAK_KB=0
  : >"$WATCH_LOG"
  (
    "$@" >>"$WATCH_LOG" 2>&1
    printf 'exit=%s\n' "$?" >>"$WATCH_LOG"
  ) &
  local runner=$! max=0 rss pgid
  while kill -0 "$runner" 2>/dev/null; do
    pgid="$(ps -o pgid= -p "$runner" 2>/dev/null | tr -d ' ')"
    rss=0
    if [ -n "$pgid" ]; then
      rss="$(ps -axo rss=,pgid= 2>/dev/null | awk -v g="$pgid" '$2==g {s+=$1} END {print s+0}')"
    fi
    if [ "$rss" -gt "$max" ]; then max="$rss"; fi
    if [ "$rss" -gt "$limit" ]; then
      kill_tree "$runner"
      printf 'WATCHDOG_KILL rss=%sKB limit=%sKB\n' "$rss" "$limit" >>"$WATCH_LOG"
      break
    fi
    sleep 5
  done
  wait "$runner" 2>/dev/null || true
  WATCH_PEAK_KB="$max"
  # Kill-exit propagation (the /tmp original never propagated a breach): a
  # watchdog kill or a missing exit line is a failure regardless of status.
  if grep -q '^WATCHDOG_KILL' "$WATCH_LOG"; then
    return 1
  fi
  local exit_line
  exit_line="$(sed -n 's/^exit=//p' "$WATCH_LOG" | tail -1)"
  [ -n "$exit_line" ] || return 1
  return "$exit_line"
}

# --- preflight ----------------------------------------------------------------

check_corpus() { # <corpus-dir>
  [ -f "$1/pubspec.yaml" ] || die "$1 is not a corpus package — assemble it first (cargo run -p harness -- corpus)"
  [ -d "$1/.dart_tool" ] || die "$1 has no .dart_tool — pub get it first (cd $1 && dart pub get; NEVER pub-get m2-warning or lints)"
}

preflight() {
  command -v jq >/dev/null 2>&1 || die "jq is required by the differential gates (brew install jq)"
  local need_dart=0 need_release=0 need_debug=0 s
  for s in "${STEPS[@]}"; do
    selected "$s" || continue
    case "$s" in
    trend-* | m1-r5 | m2-r1 | m3-r1 | lint-gate | mega-r1 | repro) need_dart=1 ;;
    esac
    case "$s" in
    trend-* | acceptance) need_release=1 ;;
    m1-r5 | m2-r1 | m3-r1 | lint-gate | mega-r1) need_debug=1 ;;
    esac
    case "$s" in
    trend-w5-A | trend-w5-B)
      check_corpus tests/corpus/m0
      check_corpus tests/corpus/m0-syntax
      [ -f tests/corpus/syntax-codes.txt ] || die "tests/corpus/syntax-codes.txt not found (the 255-code syntax universe)"
      [ -f tests/harness/m1_codes.txt ] || die "tests/harness/m1_codes.txt not found (the frozen M1 universe)"
      ;;
    m1-r5) check_corpus tests/corpus/m1-semantic ;;
    m3-r1) check_corpus tests/corpus/m3-unused ;;
    esac
  done
  if [ "$need_dart" = 1 ] && [ ! -x "$DART_BIN" ]; then
    die "fvm default dart not found at $DART_BIN (install the fvm default pin or set FVM_HOME)"
  fi
  if [ "$need_release" = 1 ] && [ ! -x "./target/release/harness" ]; then
    echo "serial-gates: NOTE ./target/release/harness missing — release-rebuild provides it" >&2
  fi
  if [ "$need_debug" = 1 ] && [ ! -x "./target/debug/harness" ]; then
    echo "serial-gates: NOTE ./target/debug/harness missing — debug-rebuild provides it" >&2
  fi
}

# --- trend runs (composed in the m0-gate.sh idiom; no trend script exists) ----

compose_union() {
  sed -e 's/#.*//' tests/corpus/syntax-codes.txt tests/harness/m1_codes.txt |
    awk '{print $1}' |
    sed 's/[[:space:]]//g' |
    grep -v '^$' |
    sort -f -u
}

trend_guard() { # <outdir> — refuse to overwrite recorded evidence
  [ -e "$1" ] && die "$1 already exists — refusing to overwrite recorded gate evidence (bump SERIAL_TAG)"
  return 0
}

trend_run() { # <syntax|union>
  local face="$1" outdir harness_args
  if [ "$face" = "syntax" ]; then
    outdir="$M0_OUT_ROOT/$LBL_TREND-syntax"
  else
    outdir="$M0_OUT_ROOT/$LBL_TREND-union"
  fi
  trend_guard "$outdir"
  [ -x ./target/release/harness ] || die "./target/release/harness not found — release-rebuild first (cargo build --release -p dartq-cli -p harness)"
  [ -x "$DART_BIN" ] || die "fvm default dart not found at $DART_BIN"
  case "$CHECK_CMD $ANALYZE_CMD" in *"./target/release/dartq"*) [ -x ./target/release/dartq ] || die "./target/release/dartq not found — release-rebuild first" ;; esac

  harness_args=(
    run
    --corpus-dir tests/corpus/m0
    --corpus-dir tests/corpus/m0-syntax
    --oracle-dart "$DART_BIN"
    --severity-filter ERROR
    --threshold 1.0
    --persist-regressions tests/corpus/regressions/cases
    --out "$outdir"
  )
  if [ "$face" = "syntax" ]; then
    watchdog_run "trend-w5-A${TAG}" "$(cap_for_step trend-w5-A)" \
      ./target/release/harness "${harness_args[@]}" \
      --dartq-cmd "$CHECK_CMD" \
      --codes-file tests/corpus/syntax-codes.txt
  else
    mkdir -p "$(dirname "$UNION_CODES")"
    compose_union >"$UNION_CODES"
    local count
    count="$(wc -l <"$UNION_CODES" | tr -d ' ')"
    [ "$count" -eq "$EXPECTED_UNION_COUNT" ] ||
      die "union code universe drifted: expected $EXPECTED_UNION_COUNT = 255 + 36 - 1 overlap, found $count (m0-gate pins this exact universe)"
    watchdog_run "trend-w5-B${TAG}" "$(cap_for_step trend-w5-B)" \
      ./target/release/harness "${harness_args[@]}" \
      --dartq-cmd "$ANALYZE_CMD" \
      --codes-file "$UNION_CODES"
  fi
}

mega_run() { # perf + parity: 1k oracle leg (parity) + scale run (perf)
  local scale="${SERIAL_MEGA_SCALE:-10k}"
  case "$scale" in 1k | 10k | 100k | all) ;; *) die "SERIAL_MEGA_SCALE must be 1k|10k|100k|all, got '$scale'" ;; esac
  [ -x ./target/debug/harness ] || die "./target/debug/harness not found — debug-rebuild first"
  if [ "${SERIAL_MEGA_ORACLE_LEG:-1}" = 1 ]; then
    watchdog_run "mega-r1-oracleleg${TAG}" "$MEGA_CAP_KB" \
      scripts/mega-run.sh --scale 1k --oracle-leg --strict --label "${LBL_MEGA}-oracleleg" || return 1
    leg_peak="$WATCH_PEAK_KB"
  fi
  if [ "$scale" = "all" ]; then
    watchdog_run "mega-r1${TAG}" "$MEGA_CAP_KB" \
      scripts/mega-run.sh --all --strict --label "$LBL_MEGA"
  else
    watchdog_run "mega-r1${TAG}" "$MEGA_CAP_KB" \
      scripts/mega-run.sh --scale "$scale" --strict --label "$LBL_MEGA"
  fi
  # Peak of the step = heaviest phase (the leg's peak survives the perf run).
  if [ -n "${leg_peak:-}" ] && [ "${leg_peak:-0}" -gt "${WATCH_PEAK_KB:-0}" ]; then
    WATCH_PEAK_KB="$leg_peak"
  fi
}

# --- per-step dispatch --------------------------------------------------------

step_runner() { # <step> — runs every watchdogged phase of the step
  case "$1" in
  release-rebuild)
    watchdog_run "$1" "$(cap_for_step "$1")" cargo build --release -p dartq-cli -p harness
    ;;
  debug-rebuild)
    watchdog_run "$1" "$(cap_for_step "$1")" cargo build -p dartq-cli -p harness
    ;;
  trend-w5-A) trend_run syntax ;;
  trend-w5-B) trend_run union ;;
  m1-r5)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/m1-gate.sh --label "$LBL_M1"
    ;;
  acceptance)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/acceptance-gate.sh --label "$LBL_ACC"
    ;;
  lint-gate)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/lint-gate.sh --label "$LBL_LINT"
    ;;
  m2-r1)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/m2-gate.sh --label "$LBL_M2"
    ;;
  m3-r1)
    watchdog_run "$1" "$(cap_for_step "$1")" env M3_LABEL="$LBL_M3" scripts/m3-gate.sh
    ;;
  mega-r1) mega_run ;;
  scanner-suite | flow-suite | parser-suite | semantic-suite | lints-suite | engine-suite)
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p "dartq-${1%-suite}" -- --test-threads 2
    ;;
  repro)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/repro-gate.sh --label "$LBL_REPRO"
    ;;
  coverage)
    local cov_args=(--stability)
    [ "${SERIAL_COVERAGE_HTML:-0}" = 1 ] && cov_args+=(--html)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/coverage-gate.sh "${cov_args[@]}"
    ;;
  esac
}

# --- run the chain ------------------------------------------------------------

fmt_wall() { # <seconds>
  printf '%dm%02ds' "$(($1 / 60))" "$(($1 % 60))"
}

fmt_mb() { # <kb>
  echo "$(($1 / 1024))MB"
}

# Run identity (runbook Phase 0.3): head + tracked-dirty note. The repro
# gate refuses a dirty tracked tree — surface that BEFORE the hours of work,
# not at the repro step.
echo "== serial-gates plan"
printf '  %-14s %s\n' "head" "$(git rev-parse HEAD)"
tracked_dirty="$(git status --porcelain | grep -cv '^??' || true)"
printf '  %-14s %s tracked file(s) modified%s\n' "worktree" "$tracked_dirty" \
  "$([ "${tracked_dirty:-0}" -gt 0 ] && echo ' — commit tracked waves before the repro step (repro-gate refuses a dirty tree)')"
printf '  %-14s %s KiB (mega-r1: %s KiB)\n' "rss cap" "$RSS_DEFAULT_KB" "$MEGA_CAP_KB"
[ -n "$ONLY_LIST" ] && printf '  %-14s %s\n' "only" "$ONLY_LIST"
[ -n "$FROM_STEP" ] && printf '  %-14s %s\n' "from" "$FROM_STEP"
[ -n "$SKIP_LIST" ] && printf '  %-14s %s\n' "skip" "$SKIP_LIST"
[ -n "$TAG" ] && printf '  %-14s %s\n' "tag" "${TAG#-}"
printf '  %-14s %s\n' "mega" "scale=${SERIAL_MEGA_SCALE:-10k} oracle_leg=${SERIAL_MEGA_ORACLE_LEG:-1}"

preflight

# Per-step records: parallel arrays (bash 3.2 — no associative arrays).
STEP_STATUS=()
STEP_WALL=()
STEP_PEAK=()
STEP_LOG=()
for s in "${STEPS[@]}"; do
  if selected "$s"; then
    STEP_STATUS+=("PENDING")
  else
    STEP_STATUS+=("SKIP")
  fi
  STEP_WALL+=("-")
  STEP_PEAK+=("-")
  STEP_LOG+=("-")
done

failed_step=""
for s in "${STEPS[@]}"; do
  i="$(index_of "$s")"
  [ "${STEP_STATUS[$i]}" = "PENDING" ] || continue
  echo
  echo "== [$s] (cap $(fmt_mb "$(cap_for_step "$s")"))"
  t0=$(date +%s)
  rc=0
  step_runner "$s" || rc=$?
  wall=$(($(date +%s) - t0))
  STEP_WALL[$i]="$(fmt_wall "$wall")"
  STEP_PEAK[$i]="$(fmt_mb "${WATCH_PEAK_KB:-0}")"
  STEP_LOG[$i]="${WATCH_LOG:-/tmp/serial_${s}${TAG}.log}"
  if [ "$rc" -eq 0 ]; then
    STEP_STATUS[$i]="PASS"
    echo "-- [$s] PASS (wall $(fmt_wall "$wall"), peak $(fmt_mb "${WATCH_PEAK_KB:-0}"), log ${STEP_LOG[$i]})"
  else
    if grep -q '^WATCHDOG_KILL' "${STEP_LOG[$i]}" 2>/dev/null; then
      STEP_STATUS[$i]="KILLED"
      echo "-- [$s] KILLED by the RSS watchdog (log ${STEP_LOG[$i]})" >&2
    else
      STEP_STATUS[$i]="FAIL(rc=$rc)"
      echo "-- [$s] FAILED (exit $rc, log ${STEP_LOG[$i]})" >&2
      tail -20 "${STEP_LOG[$i]}" 2>/dev/null | sed 's/^/  | /' >&2 || true
    fi
    failed_step="$s"
    break
  fi
done

# Steps after an abort never ran.
if [ -n "$failed_step" ]; then
  aborted=0
  for idx in "${!STEPS[@]}"; do
    if [ "${STEPS[$idx]}" = "$failed_step" ]; then
      aborted=1
      continue
    fi
    if [ "$aborted" = 1 ] && [ "${STEP_STATUS[$idx]}" = "PENDING" ]; then
      STEP_STATUS[$idx]="ABORTED"
    fi
  done
fi

echo
echo "== serial-gates summary"
printf '%-17s %-11s %-8s %-9s %s\n' "step" "status" "wall" "peak-RSS" "log"
for idx in "${!STEPS[@]}"; do
  case "${STEP_STATUS[$idx]}" in
  PASS | FAIL* | KILLED) log="${STEP_LOG[$idx]}" ;;
  *) log="-" ;;
  esac
  printf '%-17s %-11s %-8s %-9s %s\n' \
    "${STEPS[$idx]}" "${STEP_STATUS[$idx]}" "${STEP_WALL[$idx]}" "${STEP_PEAK[$idx]}" "$log"
done

if [ -n "$failed_step" ]; then
  echo "serial-gates: CHAIN FAILED at $failed_step (fix, then resume with --from / --only)"
  exit 1
fi
echo "serial-gates: CHAIN PASSED (every selected step green)"
