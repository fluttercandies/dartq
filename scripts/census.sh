#!/bin/sh
# The corpus census: dartq vs the oracle over the whole conformance corpus,
# with a staleness gate — a census taken from a stale binary is a lie about
# the distance to 1.0, so this script refuses to run unless the release
# binary is newer than every tracked source file it is built from.
#
# Usage:
#   scripts/census.sh                 full census (all cases in the manifest)
#   scripts/census.sh --subset N      smoke census over the smallest case-id
#                                     prefix covering the first N manifest
#                                     cases (single `conformance run` call)
#   scripts/census.sh --check         run the same selection twice and assert
#                                     byte-identical report + summary
#                                     (determinism gate)
#   scripts/census.sh --top N         show N divergence rows (default 25)
#
# Outputs (under target/conformance/):
#   census-<ts>.report.tsv     the per-case verdict report (run tool's shape)
#   census-<ts>.jaccard.tsv    (severity, code, file) Jaccard per severity
#                              plus OVERALL; a second section repeats the
#                              same Jaccard at full-row granularity
#                              (severity, code, file, line:col, length,
#                              message) so fabricated rows inside an
#                              otherwise-correct triple cost score; then a
#                              release_gate line (PASS/FAIL/INCOMPLETE) and
#                              the top divergence rows
#   census-<ts>.meta           binary mtime / HEAD / sdk / selection / gate
#
# A full census (no --subset) also regenerates the tracked
# conformance/status.tsv — that is the tool's designed behavior: the census
# IS the status board's source. Filtered runs leave status.tsv untouched.
#
# Environment:
#   DARTQ_BIN          dartq binary, default target/release/dartq
#   DARTQ_ORACLE_SDK   released SDK root passed to dartq as --sdk-path.
#                      Default: ~/.dartq-oracle/<pin sdk_version>/dart-sdk
#                      (the standard oracle-SDK cache layout); failing that,
#                      dartq resolves $DART_SDK.
#   DARTQ_NO_STALE_GATE  set to 1 to bypass the staleness gate (explicit
#                      escape hatch for plumbing tests; the report header
#                      then records GATE_BYPASSED — its numbers are not
#                      publishable distance-to-1.0 evidence)
#
# Notes on --subset: `conformance run` selects cases by a single id prefix,
# not by count. The script maps N to the longest common prefix of the first
# N manifest ids, which selects a superset of those N (bounded, one run).
# The actual selected count is printed and recorded in the meta file.
#
# Exit codes: 0 ok · 1 census/determinism/release-gate failure ·
#   2 usage/environment. The release gate fails (exit 1) exactly when the
#   triple-level OVERALL is 1.0 but the row-level is not — fabricated or
#   drifted rows the triple granularity masks. Sub-1.0 triples are normal
#   unfinished work (release_gate=INCOMPLETE) and keep exit 0.

set -eu

cd "$(dirname "$0")/.."

DARTQ_BIN="${DARTQ_BIN:-target/release/dartq}"
CASES_MANIFEST="conformance/cases.manifest"
EXPECTED="conformance/expected.tsv"
REPORT="target/conformance/report.tsv"

SUBSET=""
CHECK=0
TOP=25

usage() {
    sed -n '2,43p' "$0" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --subset)
            [ $# -ge 2 ] || usage
            SUBSET="$2"; shift 2
            ;;
        --subset=*)
            SUBSET="${1#--subset=}"; shift
            ;;
        --check) CHECK=1; shift ;;
        --top)
            [ $# -ge 2 ] || usage
            TOP="$2"; shift 2
            ;;
        --top=*)
            TOP="${1#--top=}"; shift
            ;;
        -h|--help) usage ;;
        *) echo "error: unknown argument $1" >&2; usage ;;
    esac
done

case "$SUBSET" in
    "") ;;
    *[!0-9]*)
        echo "error: --subset takes a positive integer (case count)" >&2
        exit 2
        ;;
    0)
        echo "error: --subset 0 selects nothing" >&2
        exit 2
        ;;
esac

# --- staleness gate -----------------------------------------------------
# The release binary must be newer than every tracked (or untracked,
# non-ignored) source file under crates/ and tools/ plus the workspace
# manifests. tests/data fixtures are excluded: they are never compiled
# into the release binary, so their edits must not trip the gate.
stale_gate() {
    [ -f "$DARTQ_BIN" ] || {
        echo "error: dartq binary $DARTQ_BIN not found; run \`cargo build --release\`" >&2
        exit 2
    }
    bin_mtime=$(stat -f %m "$DARTQ_BIN" 2>/dev/null || stat -c %Y "$DARTQ_BIN")
    git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
        echo "error: staleness gate needs a git work tree (source files are enumerated by git ls-files)" >&2
        exit 2
    }
    # git ls-files: -c tracked, -o untracked, --exclude-standard respects
    # .gitignore (target/ and .goal stay out). .cargo/config.toml and
    # rust-toolchain.toml are build inputs too.
    stale_list=$(
        git ls-files -co --exclude-standard -- \
            crates/ tools/ Cargo.toml Cargo.lock .cargo/config.toml rust-toolchain.toml |
            grep -v '/tests/data/' |
            while IFS= read -r f; do
                [ -f "$f" ] || continue
                m=$(stat -f %m "$f" 2>/dev/null || stat -c %Y "$f")
                if [ "$m" -gt "$bin_mtime" ]; then
                    printf '%s\t%s\n' "$m" "$f"
                fi
            done | sort -rn
    )
    if [ -n "$stale_list" ]; then
        newest_mtime=$(printf '%s\n' "$stale_list" | head -1 | cut -f1)
        newest_file=$(printf '%s\n' "$stale_list" | head -1 | cut -f2)
        count=$(printf '%s\n' "$stale_list" | wc -l | tr -d ' ')
        {
            echo "error: $DARTQ_BIN is stale — it predates $count source file(s) it is built from."
            echo "       binary mtime:  $(date -r "$bin_mtime" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "$bin_mtime")"
            echo "       newest source: $newest_file ($(date -r "$newest_mtime" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "$newest_mtime"))"
            echo "       A census from a stale binary misstates the distance to 1.0."
            echo "       Rebuild first:  cargo build --release -p dartq-cli"
            echo "       (plumbing-only runs: DARTQ_NO_STALE_GATE=1 — flagged in the report)"
        } >&2
        exit 2
    fi
}

if [ "${DARTQ_NO_STALE_GATE:-}" = "1" ]; then
    GATE_NOTE="GATE_BYPASSED"
    echo "warn: staleness gate bypassed (DARTQ_NO_STALE_GATE=1); numbers are not publishable" >&2
else
    stale_gate
    GATE_NOTE="gated"
fi

[ -f "$CASES_MANIFEST" ] || { echo "error: $CASES_MANIFEST not found" >&2; exit 2; }
[ -f "$EXPECTED" ] || { echo "error: $EXPECTED not found; run \`conformance oracle\` first" >&2; exit 2; }

# dartq resolves its analysis SDK from --sdk-path (DARTQ_ORACLE_SDK) or
# $DART_SDK. Default DARTQ_ORACLE_SDK to the pinned version's standard
# oracle cache when it exists; with no SDK at all every case would
# "crash", so refuse early instead of producing an all-crash census.
PIN_VERSION=$(sed -n 's/^sdk_version *= *"\(.*\)"/\1/p' conformance/pin.toml | head -1)
if [ -z "${DARTQ_ORACLE_SDK:-}" ] && [ -n "$PIN_VERSION" ]; then
    candidate="$HOME/.dartq-oracle/$PIN_VERSION/dart-sdk"
    if [ -x "$candidate/bin/dart" ]; then DARTQ_ORACLE_SDK="$candidate"; fi
fi
if [ -n "${DARTQ_ORACLE_SDK:-}" ] && [ ! -x "$DARTQ_ORACLE_SDK/bin/dart" ]; then
    echo "error: DARTQ_ORACLE_SDK=$DARTQ_ORACLE_SDK has no bin/dart" >&2
    exit 2
fi
if [ -z "${DARTQ_ORACLE_SDK:-}" ] && [ -z "${DART_SDK:-}" ]; then
    {
        echo "error: no analysis SDK. dartq needs --sdk-path to resolve against."
        echo "       Set DARTQ_ORACLE_SDK to a released SDK root (the oracle"
        echo "       expectations are pinned to sdk_version=$PIN_VERSION in"
        echo "       conformance/pin.toml), or set DART_SDK."
        echo "       Expected default location: ~/.dartq-oracle/$PIN_VERSION/dart-sdk"
    } >&2
    exit 2
fi

# --- subset → prefix -----------------------------------------------------
FILTER_ARGS=""
if [ -n "$SUBSET" ]; then
    # Longest common prefix of the first N manifest ids (the manifest is
    # sorted). `conformance run --filter` selects by prefix, so the run
    # covers a superset of the requested N — one invocation, bounded cost.
    first=$(head -1 "$CASES_MANIFEST" | cut -f1)
    nth=$(sed -n "${SUBSET}p" "$CASES_MANIFEST" | cut -f1)
    [ -n "$nth" ] || { echo "error: --subset $SUBSET exceeds the manifest" >&2; exit 2; }
    # The longest common prefix of ids[1..N] is itself the tightest --filter:
    # ids are unique and end in '#<n>', so a full id can only prefix itself —
    # --subset 1 selects exactly one case, never the whole first directory.
    prefix=$(printf '%s\n%s\n' "$first" "$nth" |
        awk 'NR==1{a=$0;next}{n=length(a);m=length($0);L=n<m?n:m;p="";for(i=1;i<=L;i++){if(substr(a,i,1)==substr($0,i,1))p=p substr(a,i,1);else break}print p}')
    if [ -z "$prefix" ]; then
        echo "error: first $SUBSET cases share no usable prefix" >&2
        exit 2
    fi
    selected=$(cut -f1 "$CASES_MANIFEST" | grep -cF "$prefix" || true)
    echo "subset: first $SUBSET manifest cases -> --filter '$prefix' ($selected cases selected)" >&2
    FILTER_ARGS="--filter $prefix"
fi

# --- one census pass -----------------------------------------------------
# Runs `conformance run` (writes the fixed target/conformance/report.tsv),
# copies it to the timestamped census name, and derives the Jaccard face.
# Filtered runs do not touch conformance/status.tsv; an unfiltered run
# regenerates it — the meta file records that.
census_pass() {
    tag="$1"   # e.g. a | b (the --check second pass)
    ts="$2"
    # shellcheck disable=SC2086
    cargo run -q -p conformance -- run --dartq "$DARTQ_BIN" \
        ${DARTQ_ORACLE_SDK:+--sdk "$DARTQ_ORACLE_SDK"} $FILTER_ARGS || {
            echo "error: conformance run failed" >&2
            return 1
        }
    out="target/conformance/census-${ts}${tag:+-$tag}.report.tsv"
    cp "$REPORT" "$out"
    summarize "$out" "target/conformance/census-${ts}${tag:+-$tag}.jaccard.tsv"
    CENSUS_REPORT="$out"
}

# --- Jaccard summary -----------------------------------------------------
# Per severity and OVERALL, at two granularities over the same expected /
# actual sets (expected = expected.tsv rows, actual = expected minus
# `missing` plus `extra` from the report — the report only records the
# diff; the matched rows live only in expected.tsv):
#   1. triple: (severity, code, file)
#   2. row:    the full canonical row text — severity|code|file|line:col|
#              length|message — so a fabricated diagnostic inside an
#              otherwise-correct triple still lands in `extra` and costs
#              score, and message/length drift is visible too.
# A release_gate line follows: PASS only when the row-level OVERALL is
# exactly 1.0 — the gate can never pass on triples alone.
# Top divergence rows: (severity, code) pairs ranked by missing+extra count.
summarize() {
    report="$1"
    jout="$2"
    # Pass 1: both granularities in one awk pass, plus a raw divergence dump
    # to a side file (sorted in pass 2). Set-level; an entry diverging in
    # either direction counts once.
    raw="$jout.raw"
    # File order: report first (its case ids bound the census into `seen`),
    # then expected.tsv restricted to those ids — a --subset run must not
    # count the whole corpus's expected rows in its union.
    awk -F'\t' -v raw="$raw" '
        function k3(sev, code, file) { return sev SUBSEP code SUBSEP file }
        # Row identity = the whole rowtext segment (messages may contain
        # "|", so the segment string itself is the key, not re-joined
        # fields). Severity for bucketing is everything before the first |.
        function rsev(rowtext) { return substr(rowtext, 1, index(rowtext, "|") - 1) }
        NR == FNR {
            # report.tsv: header then case_id \t verdict \t missing \t extra
            if (FNR == 1) next
            seen[$1] = 1
            verdict = $2
            if (verdict == "oracle_crash" || verdict == "crash") next
            # missing (col 3) ⊆ expected-only; extra (col 4) = actual-only.
            # Rows are joined by a double-semicolon separator — a message
            # containing that literal would split mid-row; its fragment
            # fails the |field| parse and is skipped at both granularities
            # (negligible: analyzer prose never uses it, and the skew is
            # one entry on one case).
            if ($3 != "") {
                nm = split($3, ms, ";;")
                for (i = 1; i <= nm; i++) {
                    if (split(ms[i], r, "|") >= 3) {
                        missing[k3(r[1], r[2], r[3])] = 1
                        rmissing[rsev(ms[i]) SUBSEP ms[i]] = 1
                        dmiss[r[1] SUBSEP r[2]]++
                    }
                }
            }
            if ($4 != "") {
                ne = split($4, es, ";;")
                for (i = 1; i <= ne; i++) {
                    if (split(es[i], r, "|") >= 3) {
                        extra[k3(r[1], r[2], r[3])] = 1
                        rextra[rsev(es[i]) SUBSEP es[i]] = 1
                        dextra[r[1] SUBSEP r[2]]++
                    }
                }
            }
            next
        }
        # expected.tsv: case_id \t - | oracle_crash | rowtext [\t rowtext]*
        !seen[$1] { next }
        $2 == "-" || $2 == "oracle_crash" { next }
        {
            for (i = 2; i <= NF; i++) {
                split($i, r, "|")
                expd[k3(r[1], r[2], r[3])] = 1
                rexpd[rsev($i) SUBSEP $i] = 1
            }
        }
        END {
            # actual = (expd − missing) ∪ extra; union = expd ∪ extra.
            # inter = actual ∩ expd: the set Jaccard of the two sides. It
            # must NOT be actual ∩ union — actual is a subset of union by
            # construction, so that form counts every fabricated entry as
            # an intersection and rewards over-reporting.
            for (k in expd) if (!(k in missing)) actual[k] = 1
            for (k in extra) actual[k] = 1
            for (k in expd) union[k] = 1
            for (k in extra) union[k] = 1
            for (k in actual) if (k in expd) {
                s = k; sub(SUBSEP ".*", "", s); inter[s]++
            }
            for (k in union) { s = k; sub(SUBSEP ".*", "", s); uni[s]++ }

            # Sorted severity order keeps the summary byte-identical
            # across awk builds (for-in iteration order is unspecified).
            print "severity\tintersection\tunion\tjaccard"
            ti = 0; tu = 0
            for (s in uni) {
                ci = inter[s] + 0; cu = uni[s]
                printf "%s\t%d\t%d\t%.6f\n", s, ci, cu, (cu ? ci / cu : 1) | "sort"
                ti += ci; tu += cu
            }
            close("sort")
            printf "OVERALL\t%d\t%d\t%.6f\n", ti, tu, (tu ? ti / tu : 1)
            triple_overall = (tu ? ti / tu : 1)

            # Row-level: same reconstruction, keys are sev SUBSEP rowtext.
            for (k in rexpd) if (!(k in rmissing)) ractual[k] = 1
            for (k in rextra) ractual[k] = 1
            for (k in rexpd) runion[k] = 1
            for (k in rextra) runion[k] = 1
            for (k in ractual) if (k in rexpd) {
                s = k; sub(SUBSEP ".*", "", s); rinter[s]++
            }
            for (k in runion) { s = k; sub(SUBSEP ".*", "", s); runi[s]++ }

            print ""
            print "# row level (full row text: severity|code|file|line:col|length|message)"
            print "severity\tintersection\tunion\tjaccard"
            rti = 0; rtu = 0
            for (s in runi) {
                ci = rinter[s] + 0; cu = runi[s]
                printf "%s\t%d\t%d\t%.6f\n", s, ci, cu, (cu ? ci / cu : 1) | "sort"
                rti += ci; rtu += cu
            }
            close("sort")
            printf "OVERALL\t%d\t%d\t%.6f\n", rti, rtu, (rtu ? rti / rtu : 1)
            row_overall = (rtu ? rti / rtu : 1)

            # The release gate is a pure function of this report: it
            # adjudicates a 1.0 claim, so it only FAILs when triples say
            # the corpus is clean but rows prove otherwise (fabrication or
            # position/message drift invisible at triple granularity).
            # A sub-1.0 triple is ordinary unfinished work, not a gate
            # violation — INCOMPLETE keeps exit 0 for the dev loop.
            print ""
            if (row_overall >= 1.0) {
                print "# release gate"
                print "release_gate\tPASS"
            } else if (triple_overall >= 1.0) {
                print "# release gate: triple level is 1.0 but row level is not — fabricated or drifted rows are masked by the triple granularity"
                print "release_gate\tFAIL"
            } else {
                print "# release gate"
                print "release_gate\tINCOMPLETE"
            }

            # Raw divergence rows for pass 2 (sorted there).
            for (c in dmiss) all[c] = 1
            for (c in dextra) all[c] = 1
            for (c in all) {
                split(c, cc, SUBSEP)
                printf "%s\t%s\t%d\t%d\t%d\n", cc[1], cc[2], dmiss[c] + 0, dextra[c] + 0, dmiss[c] + dextra[c] + 0 > raw
            }
        }
    ' "$report" "$EXPECTED" > "$jout"

    # Pass 2: append the top-N divergence table, sorted by total desc.
    {
        echo ""
        echo "# top divergence codes (missing+extra rows across the corpus)"
        echo "severity	code	missing	extra	total"
        if [ -s "$raw" ]; then
            sort -t"$(printf '\t')" -k5,5rn "$raw" | head -n "$TOP"
        fi
    } >> "$jout"
    rm -f "$raw"

    # Release gate: a FAIL line means the triple granularity masked
    # fabricated or drifted rows — that is a hard failure, not progress.
    gate=$(awk -F'\t' '$1 == "release_gate" { print $2 }' "$jout")
    if [ "$gate" = "FAIL" ]; then
        echo "RELEASE GATE: FAIL — triple-level Jaccard is 1.0 but row-level" \
            "is not; fabricated or drifted rows are masked (see $jout)" >&2
        return 1
    fi
}

# --- run -----------------------------------------------------------------
TS=$(date '+%Y%m%d-%H%M%S')

if [ "$CHECK" = "1" ]; then
    census_pass a "$TS"
    rep_a="$CENSUS_REPORT"
    jac_a="target/conformance/census-${TS}-a.jaccard.tsv"
    census_pass b "$TS"
    rep_b="$CENSUS_REPORT"
    jac_b="target/conformance/census-${TS}-b.jaccard.tsv"
    if ! cmp -s "$rep_a" "$rep_b"; then
        echo "check FAILED: reports differ between identical runs ($rep_a vs $rep_b)" >&2
        exit 1
    fi
    if ! cmp -s "$jac_a" "$jac_b"; then
        echo "check FAILED: jaccard summaries differ ($jac_a vs $jac_b)" >&2
        exit 1
    fi
    echo "check: byte-identical report + jaccard across two runs (determinism PASS)"
    CENSUS_REPORT="$rep_a"
    JACCARD_OUT="$jac_a"
else
    census_pass "" "$TS"
    JACCARD_OUT="target/conformance/census-${TS}.jaccard.tsv"
fi

# --- meta + print ---------------------------------------------------------
bin_mtime=$(stat -f %m "$DARTQ_BIN" 2>/dev/null || stat -c %Y "$DARTQ_BIN")
{
    echo "census_ts=$TS"
    echo "dartq_bin=$DARTQ_BIN"
    echo "dartq_bin_mtime=$(date -r "$bin_mtime" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "$bin_mtime")"
    echo "head=$(git rev-parse --short HEAD)"
    echo "staleness_gate=$GATE_NOTE"
    echo "sdk=${DARTQ_ORACLE_SDK:-DART_SDK=${DART_SDK:-unset}}"
    echo "filter=${FILTER_ARGS:-<full corpus>}"
    echo "check_mode=$CHECK"
    echo "report=$CENSUS_REPORT"
    echo "jaccard=$JACCARD_OUT"
    if [ "$CHECK" = "1" ]; then echo "determinism=byte-identical-2-runs"; fi
} > "target/conformance/census-${TS}.meta"

echo ""
echo "=== census $TS ($GATE_NOTE) ==="
cat "$JACCARD_OUT"
echo ""
echo "report:  $CENSUS_REPORT"
echo "jaccard: $JACCARD_OUT"
echo "meta:    target/conformance/census-${TS}.meta"
