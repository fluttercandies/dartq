# shellcheck shell=bash
# lib/perf-common.sh — measurement mechanics shared by the performance
# faces: scripts/perf-gate.sh (threshold gates) and scripts/perf-bench.sh
# (stored-baseline record/diff). One copy of the timing, RSS-sampling and
# watchdog code keeps gate numbers and bench-recorded numbers directly
# comparable.
#
# Sourcing contract: the sourcing script must (before this file) run
# `set -u -o pipefail`, cd to the repository root, and set PERF_PROG to its
# message prefix ("perf-gate" / "perf-bench").
#
# Globals read: TMP (per-run scratch dir), REALWORLD (optional override of
# conformance/realworld.toml), DARTQ_PERF_WATCHDOG_S (default 600).
# Globals written: WATCHDOG_S, DARTQ_BIN, SDK_ARGS (require_dartq_bin),
# INPUT / SKIP_REASON (resolve_analyzer_input), and the per-run measurement
# results R_RC / R_RC_OK / R_N / R_WALL / R_RSS_MIB.
#
# Platform: macOS. /usr/bin/time -l reports peak RSS in BYTES there (GNU
# time on Linux reports KB and takes -v, not -l) and the ps sampler reads
# KB; a Linux port must revisit both unit reads before its numbers mean
# anything.

PERF_PROG=${PERF_PROG:?set PERF_PROG before sourcing lib/perf-common.sh}

# die: usage/configuration error with a tutorial message. Exit 2 — part of
# the shared exit vocabulary (0 green, 1 failed/regressed, 2 usage error,
# 77 nothing measured).
die() {
    echo "$PERF_PROG: $*" >&2
    exit 2
}

WATCHDOG_S="${DARTQ_PERF_WATCHDOG_S:-600}"

# require_dartq_bin: resolve DARTQ_BIN (default the release binary) and
# SDK_ARGS from DARTQ_ORACLE_SDK, dying with the remedy when either is
# unusable.
require_dartq_bin() {
    DARTQ_BIN="${DARTQ_BIN:-target/release/dartq}"
    [ -x "$DARTQ_BIN" ] || die "no executable dartq at '$DARTQ_BIN' (build with: cargo build --release -p dartq-cli; or set DARTQ_BIN)"
    SDK_ARGS=()
    if [ -n "${DARTQ_ORACLE_SDK:-}" ]; then
        [ -d "$DARTQ_ORACLE_SDK" ] || die "DARTQ_ORACLE_SDK='$DARTQ_ORACLE_SDK' is not a directory"
        SDK_ARGS=(--sdk "$DARTQ_ORACLE_SDK")
    fi
}

# le: within-threshold float compare — le <value> <max>
le() {
    awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 <= b + 0) }'
}

# watchdog_wait <time-pid> <dartq-child-of>: wait up to WATCHDOG_S; on
# overrun kill the tree, print the regression line, return 1.
watchdog_wait() {
    local t=$1 child=$2 n=0
    while kill -0 "$t" 2>/dev/null; do
        n=$((n + 1))
        if [ "$n" -ge $((WATCHDOG_S * 2)) ]; then
            pkill -P "$child" 2>/dev/null
            kill -9 "$t" 2>/dev/null
            echo "$PERF_PROG: watchdog killed a dartq run after ${WATCHDOG_S}s (a hang is a regression)" >&2
            return 1
        fi
        sleep 0.5
    done
    return 0
}

# run_cold <input-dir> <errfile-stem>: one cold analyze; sets R_RC,
# R_WALL (seconds) and R_RSS_MIB from the /usr/bin/time -l report.
R_RC=0
R_WALL=""
R_RSS_MIB=""
run_cold() {
    local input=$1 err="$TMP/$2.time" t
    /usr/bin/time -l "$DARTQ_BIN" analyze --cold --format=json \
        ${SDK_ARGS[@]+"${SDK_ARGS[@]}"} "$input" >/dev/null 2>"$err" &
    t=$!
    R_RC=124
    R_WALL=""
    R_RSS_MIB=""
    watchdog_wait "$t" "$t" || return 0
    wait "$t"
    R_RC=$?
    R_WALL=$(awk '$2 == "real" { print $1; exit }' "$err")
    R_RSS_MIB=$(awk '/maximum resident set size/ { printf "%.1f", $1 / 1048576; exit }' "$err")
}

# tree_rss_kb <pid>...: total resident KB of each pid's tree (the pid plus
# two descendant levels: the /usr/bin/time wrapper's dartq child, and
# dartq's own children if it spawns any). macOS ps reads KB.
tree_rss_kb() {
    local out="" p d
    for p in "$@"; do
        out="$out $p"
        for d in $(pgrep -P "$p" 2>/dev/null); do
            out="$out $d $(pgrep -P "$d" 2>/dev/null | tr '\n' ' ')"
        done
    done
    out="${out# }"
    [ -n "$out" ] || {
        echo 0
        return
    }
    ps -o rss= -p "$(printf '%s' "$out" | tr -s ' ' ',')" 2>/dev/null \
        | awk '{ t += $1 } END { printf "%d", t + 0 }'
}

# run_conc <input-dir>: five parallel cold analyzes; sets R_RC_OK (0 when
# any run was killed or exited unexpectedly), R_WALL (max of the five
# "real" values) and R_RSS_MIB (peak of the 0.5 s coincident RSS sum).
R_RC_OK=0
run_conc() {
    local input=$1 i p alive s rc stuck=0
    local pids=()
    for i in 1 2 3 4 5; do
        /usr/bin/time -l "$DARTQ_BIN" analyze --cold --format=json \
            ${SDK_ARGS[@]+"${SDK_ARGS[@]}"} "$input" >/dev/null 2>"$TMP/conc$i.time" &
        pids+=($!)
    done
    local peak_kb=0 n=0
    while :; do
        alive=0
        for p in "${pids[@]}"; do kill -0 "$p" 2>/dev/null && alive=1; done
        [ "$alive" -eq 0 ] && break
        if [ "$n" -ge $((WATCHDOG_S * 2)) ]; then
            for p in "${pids[@]}"; do
                if kill -0 "$p" 2>/dev/null; then
                    pkill -P "$p" 2>/dev/null
                    kill -9 "$p" 2>/dev/null
                fi
            done
            stuck=1
            echo "$PERF_PROG: watchdog killed the parallel batch after ${WATCHDOG_S}s (a hang is a regression)" >&2
            break
        fi
        n=$((n + 1))
        s=$(tree_rss_kb "${pids[@]}")
        [ "$s" -gt "$peak_kb" ] && peak_kb=$s
        sleep 0.5
    done
    R_RC_OK=1
    [ "$stuck" = 0 ] || R_RC_OK=0
    for p in "${pids[@]}"; do
        wait "$p" 2>/dev/null
        rc=$?
        if [ "$rc" -ne 0 ] && [ "$rc" -ne 3 ]; then R_RC_OK=0; fi
    done
    R_WALL=$(awk '$2 == "real" { if ($1 + 0 > m + 0) m = $1 } END { printf "%.2f", m + 0 }' "$TMP"/conc*.time)
    R_RSS_MIB=$(awk -v k="$peak_kb" 'BEGIN { printf "%.1f", k / 1024 }')
}

# run_sum <input-root>: one cold analyze per immediate subdirectory, wall
# times summed; sets R_N (runs), R_RC_OK, R_WALL (summed), R_RSS_MIB
# (empty: a sequential RSS sum is not a footprint, so the face gates wall
# only). The results are the sourcing script's interface — shellcheck
# sees one file at a time and cannot find the reader.
R_N=0
# shellcheck disable=SC2034
run_sum() {
    local root=$1 d n=0 total=0
    R_RC_OK=1
    R_WALL=""
    R_RSS_MIB=""
    for d in "$root"/*/; do
        [ -d "$d" ] || continue
        run_cold "${d%/}" "sum$n"
        n=$((n + 1))
        if [ "$R_RC" -ne 0 ] && [ "$R_RC" -ne 3 ]; then R_RC_OK=0; fi
        if [ -n "$R_WALL" ]; then
            total=$(awk -v a="$total" -v b="$R_WALL" 'BEGIN { printf "%.2f", a + b }')
        fi
    done
    R_N=$n
    R_WALL=$total
}

# resolve_analyzer_input <face-id> <env-var-name>: set INPUT to the
# analyzer package source root — the env var when set, otherwise the
# pub-cache copy of the version pinned in conformance/realworld.toml.
# Returns 0 with INPUT set, or 1 with SKIP_REASON naming the remedy (the
# caller decides how to report the skip; a set-but-bad env var is a usage
# error here).
# shellcheck disable=SC2034
resolve_analyzer_input() {
    local id=$1 var=$2 val ver def rw
    rw=${REALWORLD:-conformance/realworld.toml}
    val=${!var:-}
    if [ -n "$val" ]; then
        [ -d "$val" ] || die "$var='$val' is not a directory (point it at the input root the TOML describes for gate '$id')"
        INPUT=$val
        return 0
    fi
    ver=$(awk '$1 == "analyzer" { v = $3; gsub(/"/, "", v); print v }' "$rw")
    if [ -z "$ver" ]; then
        SKIP_REASON="no analyzer pin found in $rw; set $var to the package source root to enable"
        return 1
    fi
    def="${PUB_CACHE:-$HOME/.pub-cache}/hosted/pub.dev/analyzer-$ver"
    if [ -d "$def" ]; then
        INPUT=$def
        return 0
    fi
    SKIP_REASON="input absent: set $var to the analyzer $ver source root, or populate the pub cache so '$def' exists (dart pub get in any workspace depending on analyzer $ver)"
    return 1
}
