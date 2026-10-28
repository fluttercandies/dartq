#!/usr/bin/env bash
# perf-gate.sh — performance-baseline regression gates over
# conformance/perf-thresholds.toml (the 2026-10-28 measurement freeze).
#
# Faces that run here: analyzer-cold (one cold analyze of the pub-cache
# analyzer package pinned in conformance/realworld.toml) and
# concurrency-5x (five parallel cold analyzes of the same input; wall =
# the batch makespan, i.e. the slowest of the five runs; peak RSS = the
# coincident five-process sum sampled every 0.5 s, the baseline protocol).
# mega100k-cold and small16-cold-sum run when their input root is provided
# through their env var; incremental-edit is always reported SKIP (it needs
# the daemon RPC edit-driver session, not a plain analyze).
#
# Never silently pass: every gate prints PASS, FAIL, or SKIP with its
# observed numbers against its threshold and baseline. Skips name the
# DARTQ_PERF_INPUT_<GATE-ID-UPPERCASED> env var (the gate's requires field
# in the TOML) that enables the face.
#
# Exit vocabulary:
#   0   every executed gate passed (skips allowed and reported)
#   1   at least one executed gate FAILED — threshold exceeded, or the
#       dartq run itself failed: a crash, an unexpected exit code, or a
#       hang killed by the watchdog (a hang is a regression, never a
#       pass)
#   2   usage/configuration error with a tutorial message: DARTQ_BIN
#       missing, or an input env var set to a nonexistent path
#  77   nothing measured: every gate skipped because its input is absent
#       (the automake "skipped" code — callers can tell "green" from
#       "not measured")
#
# Opt-in posture: performance numbers need a quiet machine. The conformance
# gate runs this script only under DARTQ_PERF=1. On a loaded host the wall
# thresholds can trip on scheduling noise (thresholds are regression
# tripwires, baseline x1.25 wall / x1.15 RSS — the slack rationale lives in
# the TOML header). Re-run on an idle machine before acting on a wall-face
# failure, and never loosen a threshold to make a noisy run pass.
#
# Watchdog: every dartq run is killed after DARTQ_PERF_WATCHDOG_S seconds
# (default 600; the slowest baselined face needs ~29) and the gate FAILS —
# the baseline itself caught a dartq panic+hang, so a stuck run must trip
# the gate instead of stalling it.
#
# Platform: macOS. /usr/bin/time -l reports peak RSS in BYTES there (GNU
# time on Linux reports KB and takes -v, not -l) and the ps sampler reads
# KB; a Linux port must revisit both unit reads before its PASS means
# anything.
#
# SDK resolution mirrors scripts/conformance-gate.sh: DARTQ_ORACLE_SDK is
# passed to dartq as --sdk when set; otherwise dartq resolves $DART_SDK.
#
# The measurement mechanics (the watchdog, the /usr/bin/time -l parsing,
# the coincident-RSS sampler, the analyzer input resolution) live in
# scripts/lib/perf-common.sh, shared with scripts/perf-bench.sh — one copy
# keeps gate numbers and bench-recorded numbers directly comparable.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

THRESHOLDS=conformance/perf-thresholds.toml
REALWORLD=conformance/realworld.toml
PERF_PROG=perf-gate
. scripts/lib/perf-common.sh

[ -f "$THRESHOLDS" ] || die "missing $THRESHOLDS"
require_dartq_bin

TMP=$(mktemp -d "${TMPDIR:-/tmp}/perf-gate.XXXXXX") || die "mktemp failed"
trap 'rm -rf "$TMP"' EXIT

# ---- parse the gate table ------------------------------------------------
# One awk pass emits "id<TAB>key<TAB>value" rows for the documented subset
# of keys; entries are "key = value", one per line. Kept bash-3.2 clean:
# parallel indexed arrays, no associative arrays.
GATE_IDS=()
GATE_REQ=()
GATE_W50=()
GATE_W95=()
GATE_RSS=()
GATE_BW=()
GATE_BRS=()

idx_of() {
    local i
    for ((i = 0; i < ${#GATE_IDS[@]}; i++)); do
        [ "${GATE_IDS[$i]}" = "$1" ] && {
            echo "$i"
            return
        }
    done
    echo -1
}

while IFS=$'\t' read -r gid key value; do
    [ -n "$gid" ] || continue
    i=$(idx_of "$gid")
    if [ "$i" = -1 ]; then
        GATE_IDS+=("$gid")
        i=$((${#GATE_IDS[@]} - 1))
        GATE_REQ+=("")
        GATE_W50+=("")
        GATE_W95+=("")
        GATE_RSS+=("")
        GATE_BW+=("")
        GATE_BRS+=("")
    fi
    case "$key" in
    requires) GATE_REQ[i]=$value ;;
    wall_p50_max_s) GATE_W50[i]=$value ;;
    wall_p95_max_s) GATE_W95[i]=$value ;;
    rss_max_mib) GATE_RSS[i]=$value ;;
    baseline_wall_s) GATE_BW[i]=$value ;;
    baseline_rss_mib) GATE_BRS[i]=$value ;;
    esac
done < <(awk '
    /^\[\[gate\]\]/ { ingate = 1; gid = ""; next }
    /^\[/           { ingate = 0; next }
    !ingate || !/=/{ next }
    {
        key = $1
        sub(/=.*/, "", key)
        value = $0
        sub(/^[^=]*=[ \t]*/, "", value)
        gsub(/"/, "", value)
        if (key == "id") { gid = value; next }
        if (gid == "") next
        if (key == "requires" || key == "wall_p50_max_s" || key == "wall_p95_max_s" ||
            key == "rss_max_mib" || key == "baseline_wall_s" || key == "baseline_rss_mib")
            printf "%s\t%s\t%s\n", gid, key, value
    }
' "$THRESHOLDS")
[ "${#GATE_IDS[@]}" -gt 0 ] || die "no [[gate]] entries parsed from $THRESHOLDS"

ran=0
failed=0
skipped=0

report() { # <id> <PASS|FAIL|SKIP> <detail>
    printf 'perf-gate: %-16s %-4s %s\n' "$1" "$2" "$3"
    case "$2" in
    PASS) ran=$((ran + 1)) ;;
    FAIL)
        ran=$((ran + 1))
        failed=$((failed + 1))
        ;;
    SKIP) skipped=$((skipped + 1)) ;;
    esac
}

skip_gate() { # <id> <reason>
    report "$1" SKIP "$2"
}

verdict() { # <ok-0|1> -> echoes PASS / FAIL
    [ "$1" = 1 ] && echo PASS || echo FAIL
}

# ---- input resolution ----------------------------------------------------
# Env var first (the gate's requires field). Unset: the analyzer-shaped
# gates fall back to the pub-cache package pinned in conformance/
# realworld.toml; anything still unresolvable is an explicit SKIP naming
# the remedy — never a silent pass. Set-but-bad is a usage error.
INPUT=""
resolve_input() { # <gate-index>; returns 1 when the gate must SKIP
    local i=$1 id=${GATE_IDS[$1]} var val
    var=${GATE_REQ[$i]}
    [ -n "$var" ] || var="DARTQ_PERF_INPUT_$(printf '%s' "$id" | tr '[:lower:]-' '[:upper:]_')"
    val=${!var:-}
    if [ -n "$val" ]; then
        [ -d "$val" ] || die "$var='$val' is not a directory (point it at the input root the TOML describes for gate '$id')"
        INPUT=$val
        return 0
    fi
    case "$id" in
    analyzer-cold | concurrency-5x)
        if resolve_analyzer_input "$id" "$var"; then
            return 0
        fi
        skip_gate "$id" "$SKIP_REASON"
        return 1
        ;;
    mega100k-cold)
        skip_gate "$id" "input absent: set $var to the mega/100k corpus root to enable"
        return 1
        ;;
    small16-cold-sum)
        skip_gate "$id" "input absent: set $var to a directory holding the 16 packages as subdirectories to enable"
        return 1
        ;;
    incremental-edit)
        skip_gate "$id" "not runnable by perf-gate.sh: needs the daemon RPC edit-driver session (the A6 protocol); thresholds stay declared in $THRESHOLDS"
        return 1
        ;;
    *)
        skip_gate "$id" "no runner for gate id '$id'; thresholds stay declared in $THRESHOLDS"
        return 1
        ;;
    esac
}

# ---- gate drivers --------------------------------------------------------
gate_single() { # <index> — analyzer-cold / mega100k-cold shape
    local i=$1 id=${GATE_IDS[$1]} ok=1 msg=""
    run_cold "$INPUT" "$id"
    if [ "$R_RC" -ne 0 ] && [ "$R_RC" -ne 3 ]; then
        ok=0
        msg="dartq exited rc=$R_RC (expected 0 clean or 3 with diagnostics); "
    fi
    if [ -z "$R_WALL" ]; then
        ok=0
        msg="${msg}no wall report; "
    elif ! le "$R_WALL" "${GATE_W50[$i]}"; then
        ok=0
        msg="${msg}wall over threshold; "
    fi
    if [ -n "$R_RSS_MIB" ] && [ -n "${GATE_RSS[$i]}" ] && ! le "$R_RSS_MIB" "${GATE_RSS[$i]}"; then
        ok=0
        msg="${msg}peak RSS over threshold; "
    fi
    report "$id" "$(verdict "$ok")" \
        "wall ${R_WALL:-?}s / max ${GATE_W50[$i]}s / base ${GATE_BW[$i]}s | peak RSS ${R_RSS_MIB:-?} / max ${GATE_RSS[$i]:-n.g.} / base ${GATE_BRS[$i]:-n.g.} MiB${msg:+ | $msg}"
}

gate_conc() { # <index> — concurrency-5x shape
    local i=$1 id=${GATE_IDS[$1]} ok=1 msg=""
    run_conc "$INPUT"
    [ "$R_RC_OK" = 1 ] || {
        ok=0
        msg="a parallel run died unexpectedly; "
    }
    if ! le "$R_WALL" "${GATE_W50[$i]}"; then
        ok=0
        msg="${msg}makespan over threshold; "
    fi
    if [ -n "${GATE_RSS[$i]}" ] && ! le "$R_RSS_MIB" "${GATE_RSS[$i]}"; then
        ok=0
        msg="${msg}coincident RSS sum over threshold; "
    fi
    report "$id" "$(verdict "$ok")" \
        "makespan ${R_WALL}s / max ${GATE_W50[$i]}s / base ${GATE_BW[$i]}s | coincident RSS ${R_RSS_MIB} / max ${GATE_RSS[$i]} / base ${GATE_BRS[$i]} MiB${msg:+ | $msg}"
}

gate_sum() { # <index> — small16-cold-sum shape
    local i=$1 id=${GATE_IDS[$1]} ok=1 msg=""
    run_sum "$INPUT"
    [ "$R_N" -gt 0 ] || {
        ok=0
        msg="input root has no package subdirectories; "
    }
    [ "$R_RC_OK" = 1 ] || {
        ok=0
        msg="${msg}a run died unexpectedly; "
    }
    if [ "$R_N" -gt 0 ] && [ -n "$R_WALL" ] && ! le "$R_WALL" "${GATE_W50[$i]}"; then
        ok=0
        msg="${msg}wall sum over threshold; "
    fi
    report "$id" "$(verdict "$ok")" \
        "$R_N runs, wall sum ${R_WALL:-?}s / max ${GATE_W50[$i]}s / base ${GATE_BW[$i]}s (RSS not gated on this face)${msg:+ | $msg}"
}

for gi in "${!GATE_IDS[@]}"; do
    resolve_input "$gi" || continue
    case ${GATE_IDS[$gi]} in
    analyzer-cold | mega100k-cold) gate_single "$gi" ;;
    concurrency-5x) gate_conc "$gi" ;;
    small16-cold-sum) gate_sum "$gi" ;;
    esac
done

echo "perf-gate: summary: $ran ran, $failed failed, $skipped skipped"
if [ "$ran" -eq 0 ]; then
    echo "perf-gate: nothing measured (every gate skipped for absent inputs)" >&2
    exit 77
fi
if [ "$failed" -gt 0 ]; then
    exit 1
fi
exit 0
