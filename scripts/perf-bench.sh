#!/usr/bin/env bash
# perf-bench.sh — stored-baseline performance bench: record measured runs
# into a local history, then diff later runs against a stored reference,
# reporting per-face deltas (improvements and regressions) instead of only
# threshold trips — the rust-analyzer xtask benchmark precedent, closing
# the gap where a passing perf-gate says nothing about gradual drift.
#
# Faces: the two perf-gate.sh can always run — analyzer-cold (one cold
# analyze of the pub-cache analyzer package at the version pinned in
# conformance/realworld.toml) and concurrency-5x (five parallel cold
# analyzes; wall = the batch makespan, RSS = the coincident five-process
# sum). Measurement mechanics are shared with perf-gate.sh through
# scripts/lib/perf-common.sh, so recorded numbers and gate numbers are
# directly comparable.
#
# Modes:
#   scripts/perf-bench.sh record
#       Measure both faces and append one row per face to
#       target/perf-history/records.tsv (untracked, machine-local):
#       date, git sha, face, wall seconds, peak RSS MiB. Rows land only
#       when BOTH faces measured cleanly — a failed run is never recorded.
#   scripts/perf-bench.sh diff [--against latest|baseline|<git-sha>]
#       Measure both faces again and print a per-face delta table against
#       a stored reference: "latest" — the newest matching row in
#       records.tsv (the default); "baseline" — the tracked quiet-machine
#       reference in conformance/perf-baseline.tsv; or the newest record
#       row whose git sha starts with the given prefix.
#
# Regression rule: a metric fails when the new wall exceeds the reference
# wall x wall_slack or the new peak RSS exceeds the reference RSS x
# rss_slack. The slack factors are read from the [meta] section of
# conformance/perf-thresholds.toml — the same noise envelope the absolute
# gate thresholds there were derived from, taken from the TOML so this
# script holds no second copy. DARTQ_PERF_DIFF_SLACK_WALL /
# DARTQ_PERF_DIFF_SLACK_RSS override a factor for one run (validated
# positive; mainly to exercise the failure path deterministically).
#
# Exit vocabulary (mirrors perf-gate.sh):
#   0   measured; no regression beyond slack (deltas reported either way)
#   1   a regression beyond slack, or the measurement itself failed: a
#       dartq crash, an unexpected exit code, or a watchdog kill (a hang
#       is a regression, never a pass)
#   2   usage/configuration error: unknown mode or flag, missing or
#       malformed store, a --against sha matching no record, or a
#       non-positive slack factor
#  77   nothing measured: the analyzer input root is absent (the remedy
#       env var is named, mirroring the perf-gate skip posture)
#
# Posture: performance numbers need a quiet machine. On a shared host a
# wall regression can be scheduling noise — re-run on an idle machine
# before acting, and never loosen a slack factor to make noise pass.
# records.tsv is machine-local state (target/ is not tracked); the
# durable, reviewable reference is conformance/perf-baseline.tsv,
# refreshed by recording on a quiet machine and promoting the rows (the
# procedure is documented in its header).
#
# Platform: macOS (same /usr/bin/time -l and ps unit reads as
# perf-gate.sh; a Linux port must revisit both before its numbers mean
# anything).
#
# SDK resolution mirrors perf-gate.sh: DARTQ_ORACLE_SDK is passed to dartq
# as --sdk when set; otherwise dartq resolves $DART_SDK.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

PERF_PROG=perf-bench
. scripts/lib/perf-common.sh

THRESHOLDS=conformance/perf-thresholds.toml
BASELINE=conformance/perf-baseline.tsv
HISTORY_DIR=target/perf-history
RECORDS=$HISTORY_DIR/records.tsv

usage() {
    cat >&2 <<'EOF'
usage: scripts/perf-bench.sh record
       scripts/perf-bench.sh diff [--against latest|baseline|<git-sha>]

  record  measure both faces, append rows to target/perf-history/records.tsv
  diff    measure both faces again, print per-face deltas vs a reference:
          latest — the newest record row (default), baseline — the tracked
          conformance/perf-baseline.tsv, or the newest record row whose
          git sha starts with the given prefix

Exit: 0 no regression | 1 regression or failed measurement |
      2 usage/configuration error | 77 input absent, nothing measured
EOF
    exit 2
}

mode=${1:-}
[ -n "$mode" ] || usage
if [ "$#" -gt 0 ]; then shift; fi
against=latest
case "$mode" in
record)
    [ "$#" -eq 0 ] || usage
    ;;
diff)
    if [ "$#" -gt 0 ]; then
        { [ "$1" = "--against" ] && [ "$#" -eq 2 ]; } || usage
        against=$2
    fi
    ;;
*) usage ;;
esac

[ -f "$THRESHOLDS" ] || die "missing $THRESHOLDS"

# meta_of <key> <toml>: the value of a scalar "key = value" entry in the
# file's [meta] section (the same minimal one-entry-per-line TOML reading
# perf-gate's gate parser uses).
meta_of() {
    awk -v key="$1" '
        /^\[meta\]/ { inmeta = 1; next }
        /^\[/       { inmeta = 0; next }
        inmeta && index($0, key " = ") == 1 {
            v = $0
            sub(/^[^=]*=[ \t]*/, "", v)
            gsub(/"/, "", v)
            print v
            exit
        }
    ' "$2"
}

positive() { awk -v v="$1" 'BEGIN { exit !(v + 0 > 0) }'; }

# Slack factors: from the TOML [meta], never a second copy here. The env
# overrides exist to exercise the failure path deterministically and for
# controlled what-if runs; both are validated like the TOML values.
WALL_SLACK=$(meta_of wall_slack "$THRESHOLDS")
RSS_SLACK=$(meta_of rss_slack "$THRESHOLDS")
[ -n "$WALL_SLACK" ] && [ -n "$RSS_SLACK" ] ||
    die "no wall_slack/rss_slack in [meta] of $THRESHOLDS (the diff slack must come from the TOML, not a copy here)"
positive "$WALL_SLACK" || die "wall_slack='$WALL_SLACK' in $THRESHOLDS is not a positive number"
positive "$RSS_SLACK" || die "rss_slack='$RSS_SLACK' in $THRESHOLDS is not a positive number"
if [ -n "${DARTQ_PERF_DIFF_SLACK_WALL:-}" ]; then
    positive "$DARTQ_PERF_DIFF_SLACK_WALL" ||
        die "DARTQ_PERF_DIFF_SLACK_WALL='$DARTQ_PERF_DIFF_SLACK_WALL' is not a positive number"
    WALL_SLACK=$DARTQ_PERF_DIFF_SLACK_WALL
fi
if [ -n "${DARTQ_PERF_DIFF_SLACK_RSS:-}" ]; then
    positive "$DARTQ_PERF_DIFF_SLACK_RSS" ||
        die "DARTQ_PERF_DIFF_SLACK_RSS='$DARTQ_PERF_DIFF_SLACK_RSS' is not a positive number"
    RSS_SLACK=$DARTQ_PERF_DIFF_SLACK_RSS
fi

require_dartq_bin

TMP=$(mktemp -d "${TMPDIR:-/tmp}/perf-bench.XXXXXX") || die "mktemp failed"
trap 'rm -rf "$TMP"' EXIT

# fail_run: the run/measurement failure exit (1) — distinct from die's
# usage/configuration exit (2).
fail_run() {
    echo "perf-bench: $*" >&2
    exit 1
}

A_WALL=""
A_RSS=""
C_WALL=""
C_RSS=""

# measure_faces: run both faces over the shared input; sets A_WALL/A_RSS
# (analyzer-cold) and C_WALL/C_RSS (concurrency-5x), or exits — 77 when
# the input root is absent, 1 when a dartq run failed or a report is
# missing. A partial measurement never reaches record or diff.
measure_faces() {
    resolve_analyzer_input analyzer-cold DARTQ_PERF_INPUT_ANALYZER_COLD ||
        { echo "perf-bench: nothing measured: $SKIP_REASON (both faces share this input)" >&2
            exit 77
        }
    run_cold "$INPUT" analyzer-cold
    { [ "$R_RC" -eq 0 ] || [ "$R_RC" -eq 3 ]; } ||
        fail_run "analyzer-cold: dartq exited rc=$R_RC (expected 0 clean or 3 with diagnostics)"
    [ -n "$R_WALL" ] || fail_run "analyzer-cold: no wall report from /usr/bin/time"
    [ -n "$R_RSS_MIB" ] || fail_run "analyzer-cold: no peak-RSS report from /usr/bin/time"
    A_WALL=$R_WALL
    A_RSS=$R_RSS_MIB
    run_conc "$INPUT"
    [ "$R_RC_OK" = 1 ] ||
        fail_run "concurrency-5x: a parallel run died unexpectedly or was watchdog-killed"
    [ -n "$R_WALL" ] || fail_run "concurrency-5x: no wall report"
    [ -n "$R_RSS_MIB" ] || fail_run "concurrency-5x: no coincident-RSS report"
    C_WALL=$R_WALL
    C_RSS=$R_RSS_MIB
}

record_mode() {
    local ts sha
    ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    sha=$(git rev-parse --short=12 HEAD) ||
        die "git rev-parse failed (records are keyed by commit; run inside the repository)"
    measure_faces
    mkdir -p "$HISTORY_DIR" || die "cannot create $HISTORY_DIR"
    [ -f "$RECORDS" ] || printf '# date\tcommit\tface\twall_s\trss_mib\n' >"$RECORDS" ||
        die "cannot write $RECORDS"
    printf '%s\t%s\tanalyzer-cold\t%s\t%s\n' "$ts" "$sha" "$A_WALL" "$A_RSS" >>"$RECORDS" ||
        die "cannot append to $RECORDS"
    printf '%s\t%s\tconcurrency-5x\t%s\t%s\n' "$ts" "$sha" "$C_WALL" "$C_RSS" >>"$RECORDS" ||
        die "cannot append to $RECORDS"
    echo "perf-bench: recorded to $RECORDS"
    printf 'perf-bench:   analyzer-cold   wall %ss rss %s MiB\n' "$A_WALL" "$A_RSS"
    printf 'perf-bench:   concurrency-5x  wall %ss rss %s MiB (makespan / coincident sum)\n' "$C_WALL" "$C_RSS"
}

# last_row <file> <face> <sha-filter>: print the last row for the face
# (rows whose sha starts with the filter when one is given). Awk exits 1
# on no match and 2 on a malformed row — a broken store must fail loudly,
# never diff against garbage; a row must have exactly five populated
# fields (the writer guarantees it).
last_row() {
    awk -v face="$2" -v sha="$3" '
        /^#/ { next }
        NF != 5 {
            printf "perf-bench: malformed row (want date, commit, face, wall_s, rss_mib): %s\n", $0 > "/dev/stderr"
            bad = 1
            next
        }
        (sha == "" || index($2, sha) == 1) && $3 == face { line = $0 }
        END {
            if (bad) exit 2
            if (line != "") { print line; exit 0 }
            exit 1
        }
    ' "$1"
}

REF_A_DATE="" REF_A_SHA="" REF_A_WALL="" REF_A_RSS=""
REF_C_DATE="" REF_C_SHA="" REF_C_WALL="" REF_C_RSS=""

# load_ref <file> <sha-filter>: fill the REF_* globals for both faces from
# the store, validating the numbers the delta math will divide by.
load_ref() {
    local file=$1 filter=$2 row d sha face wall rss where
    row=$(last_row "$file" analyzer-cold "$filter") || {
        where="no reference row for analyzer-cold in $file"
        [ -n "$filter" ] && where="$where (sha prefix '$filter')"
        die "$where — record first, or use --against baseline"
    }
    IFS=$'\t' read -r d sha face wall rss <<<"$row"
    REF_A_DATE=$d REF_A_SHA=$sha REF_A_WALL=$wall REF_A_RSS=$rss
    positive "$REF_A_WALL" || die "reference row for analyzer-cold in $file has a non-positive wall: $row"
    positive "$REF_A_RSS" || die "reference row for analyzer-cold in $file has a non-positive rss: $row"
    row=$(last_row "$file" concurrency-5x "$filter") || {
        where="no reference row for concurrency-5x in $file"
        [ -n "$filter" ] && where="$where (sha prefix '$filter')"
        die "$where — record first, or use --against baseline"
    }
    IFS=$'\t' read -r d sha face wall rss <<<"$row"
    REF_C_DATE=$d REF_C_SHA=$sha REF_C_WALL=$wall REF_C_RSS=$rss
    positive "$REF_C_WALL" || die "reference row for concurrency-5x in $file has a non-positive wall: $row"
    positive "$REF_C_RSS" || die "reference row for concurrency-5x in $file has a non-positive rss: $row"
}

FAILURES=0

# metric_row <face> <metric> <new> <ref> <slack>: print one delta-table
# row; count a REGRESSION (new beyond ref x slack) into FAILURES.
metric_row() {
    local face=$1 metric=$2 new=$3 ref=$4 slack=$5 out verdict delta
    out=$(awk -v n="$new" -v r="$ref" -v s="$slack" 'BEGIN {
        printf "%+.1f\t%s\n", (n - r) / r * 100, (n > r * s ? "REGRESSION" : (n < r ? "improved" : "ok"))
    }')
    delta=${out%%$'\t'*}
    verdict=${out#*$'\t'}
    if [ "$verdict" = REGRESSION ]; then
        FAILURES=$((FAILURES + 1))
    fi
    printf '%-16s %-8s %10s %10s %9s%%  %s\n' "$face" "$metric" "$new" "$ref" "$delta" "$verdict"
}

diff_mode() {
    case "$against" in
    latest)
        [ -f "$RECORDS" ] ||
            die "no records yet at $RECORDS — run 'scripts/perf-bench.sh record' first, or diff against the tracked quiet-machine reference with '--against baseline'"
        load_ref "$RECORDS" ""
        echo "perf-bench: diff vs the newest record in $RECORDS (slack: wall x$WALL_SLACK, RSS x$RSS_SLACK — [meta] of $THRESHOLDS)"
        ;;
    baseline)
        [ -f "$BASELINE" ] || die "missing $BASELINE"
        load_ref "$BASELINE" ""
        echo "perf-bench: diff vs the tracked baseline $BASELINE (slack: wall x$WALL_SLACK, RSS x$RSS_SLACK — [meta] of $THRESHOLDS)"
        ;;
    *)
        [ -f "$RECORDS" ] ||
            die "--against '$against': no records at $RECORDS to match a sha prefix against"
        load_ref "$RECORDS" "$against"
        echo "perf-bench: diff vs records matching sha prefix '$against' in $RECORDS (slack: wall x$WALL_SLACK, RSS x$RSS_SLACK — [meta] of $THRESHOLDS)"
        ;;
    esac
    echo "perf-bench: reference rows — analyzer-cold $REF_A_DATE $REF_A_SHA, concurrency-5x $REF_C_DATE $REF_C_SHA"
    printf '%-16s %-8s %10s %10s %10s  %s\n' face metric new ref delta verdict
    measure_faces
    metric_row analyzer-cold wall_s "$A_WALL" "$REF_A_WALL" "$WALL_SLACK"
    metric_row analyzer-cold rss_mib "$A_RSS" "$REF_A_RSS" "$RSS_SLACK"
    metric_row concurrency-5x wall_s "$C_WALL" "$REF_C_WALL" "$WALL_SLACK"
    metric_row concurrency-5x rss_mib "$C_RSS" "$REF_C_RSS" "$RSS_SLACK"
    if [ "$FAILURES" -gt 0 ]; then
        echo "perf-bench: $FAILURES metric(s) regressed beyond slack" >&2
        exit 1
    fi
    echo "perf-bench: no regression beyond slack"
    exit 0
}

case "$mode" in
record) record_mode ;;
diff) diff_mode ;;
esac
