#!/usr/bin/env bash
# oracle_baseline.sh — repeatable cold/hot analysis baseline benchmark (goal
# #102; generalized for the A7 dartq-side leg, #798).
#
# Measures wall-clock time and peak RSS of `dart analyze --format=machine <dir>`
# over N rounds (default 5), plus corpus scale (files / LOC / bytes).
# Emits one JSON object per line: one per round, then one summary record.
#
# Caliber notes:
# - "cold" = round 1 (first invocation of this measurement session). True page-cache
#   purge is not possible on macOS without root (`/usr/sbin/purge` requires root),
#   so cold vs hot captures analyzer-internal cost + VM startup, not disk cache state.
#   Rounds 2..N are "hot" (same session, warm file cache).
# - Single-instance discipline: rounds run strictly sequentially; do not run this
#   script concurrently with another analysis process on either side (noisy numbers).
# - The script never runs `pub get`; the target directory must already have a
#   package context (or none — dart analyze also works without one, see
#   the oracle machine-format contract).
#
# Usage: oracle_baseline.sh <target-dir> [rounds] [--label <name>] [--cmd "<command>"]
#
#   --cmd "<command>"  run <command> instead of the default `dart analyze
#                      --format=machine` (the A7 dartq-side leg). One argument,
#                      shell-word-split; the target dir is appended as the final
#                      argument. The summary then records "cmd" (verbatim) and
#                      "tool_version" (the command's binary `--version` banner —
#                      one spawn per invocation, never per round) in place of
#                      the "dart" field; every other field is unchanged, so a
#                      --cmd leg and the default leg share one JSON schema
#                      apart from that descriptor swap.
#   --label <name>     summary "target" name (default: the target dir basename).
set -uo pipefail

DIR=""
ROUNDS=""
LABEL=""
CMD=""
while [ $# -gt 0 ]; do
  case "$1" in
  --label)
    [ $# -ge 2 ] || { echo "error: --label needs a value" >&2; exit 2; }
    LABEL="$2"
    shift 2
    ;;
  --cmd)
    [ $# -ge 2 ] || { echo "error: --cmd needs a value" >&2; exit 2; }
    CMD="$2"
    shift 2
    ;;
  -h | --help)
    sed -n '2,/^set -uo pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    if [ -z "$DIR" ]; then
      DIR="$1"
    elif [ -z "$ROUNDS" ]; then
      ROUNDS="$1"
    else
      echo "error: unexpected argument: $1" >&2
      exit 2
    fi
    shift
    ;;
  esac
done
[ -n "$DIR" ] ||
  { echo "error: usage: oracle_baseline.sh <target-dir> [rounds] [--label <name>] [--cmd \"<command>\"]" >&2; exit 2; }
ROUNDS="${ROUNDS:-5}"
LABEL="${LABEL:-$(basename "$DIR")}"

[ -d "$DIR" ] || { echo "error: not a directory: $DIR" >&2; exit 2; }

# The timed command: default `dart analyze --format=machine <dir>` (argv
# identical to the pre-#798 shape), or the --cmd words with the target dir
# appended. Word-splitting via eval handles quoted paths in the command.
if [ -z "$CMD" ]; then
  command -v dart >/dev/null || { echo "error: dart not on PATH" >&2; exit 2; }
  RUN_WORDS=(dart analyze --format=machine "$DIR")
else
  eval "RUN_WORDS=($CMD)" 2>/dev/null || { echo "error: cannot parse --cmd: $CMD" >&2; exit 2; }
  [ "${#RUN_WORDS[@]}" -ge 1 ] || { echo "error: --cmd is empty" >&2; exit 2; }
  command -v "${RUN_WORDS[0]}" >/dev/null 2>&1 ||
    { echo "error: --cmd binary not found or not executable: ${RUN_WORDS[0]}" >&2; exit 2; }
  # The --cmd words get the target dir appended (the documented contract):
  # callers pass the tool prefix only; the corpus is always "$DIR".
  RUN_WORDS+=("$DIR")
fi

# --- corpus scale -----------------------------------------------------------
FILES=$(find "$DIR" -name '*.dart' | wc -l | tr -d ' ')
LOC=$(find "$DIR" -name '*.dart' -exec cat {} + 2>/dev/null | wc -l | tr -d ' ')
BYTES=$(find "$DIR" -name '*.dart' -exec stat -f%z {} + | awk '{s+=$1} END {print s}')

# Package context kind (recorded, never modified).
if [ -f "$DIR/.dart_tool/package_config.json" ]; then CONTEXT="package_config"
elif [ -f "$DIR/pubspec.yaml" ]; then CONTEXT="pubspec_only"
else CONTEXT="none"; fi

# --- environment descriptor -------------------------------------------------
# The tool version: the default leg records the dart banner ("dart" field);
# a --cmd leg records the command's own binary banner ("tool_version") — one
# spawn per invocation either way, never per round.
DART_VER=""
TOOL_VER=""
if [ -z "$CMD" ]; then
  DART_VER=$(dart --version 2>&1)
else
  TOOL_VER=$("${RUN_WORDS[0]}" --version 2>&1)
fi
OS_VER=$(sw_vers -productVersion)
ARCH=$(uname -m)
CHIP=$(sysctl -n machdep.cpu.brand_string)
CORES=$(sysctl -n hw.ncpu)
RAM_BYTES=$(sysctl -n hw.memsize)
STAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

json_escape() { local s=$1; s=${s//\\/\\\\}; s=${s//\"/\\\"}; printf '%s' "$s"; }

# --- timed rounds -----------------------------------------------------------
# One timed run per round. `/usr/bin/time -l -o` puts the time report in its own
# file. macOS time format is value-first: "0.71 real ..." and
# "6684672  maximum resident set size" — hence $1, not $NF.
for i in $(seq 1 "$ROUNDS"); do
  PHASE=$([ "$i" -eq 1 ] && echo cold || echo hot)
  set +e
  /usr/bin/time -l -o "$WORK/time.$i" "${RUN_WORDS[@]}" \
    >"$WORK/out.$i" 2>"$WORK/err.$i" </dev/null
  CODE=$?
  REAL=$(awk '/ real / {printf "%.0f", $1 * 1000}' "$WORK/time.$i")
  MAXRSS=$(awk '/maximum resident set size/ {print $1}' "$WORK/time.$i")
  DIAGS=$(wc -l <"$WORK/out.$i" | tr -d ' ')
  ERRBYTES=$(wc -c <"$WORK/err.$i" | tr -d ' ')
  printf '{"record":"round","target":"%s","context":"%s","round":%d,"phase":"%s","real_ms":%s,"maxrss_bytes":%s,"exit_code":%d,"diag_lines":%s,"stderr_bytes":%s}\n' \
    "$LABEL" "$CONTEXT" "$i" "$PHASE" "$REAL" "$MAXRSS" "$CODE" "$DIAGS" "$ERRBYTES"
done

# --- summary (round 1 = cold; min/median over hot rounds 2..N) ---------------
REALS=$(for i in $(seq 1 "$ROUNDS"); do
  awk '/ real / {printf "%.0f\n", $1 * 1000}' "$WORK/time.$i"
done)
RSS_ALL=$(for i in $(seq 1 "$ROUNDS"); do
  awk '/maximum resident set size/ {print $1}' "$WORK/time.$i"
done)
COLD_MS=$(echo "$REALS" | head -1)
HOT_SORTED=$(echo "$REALS" | tail -n +2 | sort -n)
HOT_N=$((ROUNDS - 1))
HOT_MIN=$(echo "$HOT_SORTED" | head -1)
HOT_MEDIAN=$(echo "$HOT_SORTED" | awk -v n="$HOT_N" 'NR == int((n + 1) / 2) {print; exit}')
RSS_MEDIAN=$(echo "$RSS_ALL" | sort -n | awk -v n="$ROUNDS" 'NR == int((n + 1) / 2) {print; exit}')
DIAGS1=$(wc -l <"$WORK/out.1" | tr -d ' ')

# Default leg: the pre-#798 summary shape, byte-identical.
if [ -z "$CMD" ]; then
  printf '{"record":"summary","target":"%s","context":"%s","files":%s,"loc":%s,"bytes":%s,"rounds":%d,"cold_real_ms":%s,"hot_min_ms":%s,"hot_median_ms":%s,"maxrss_median_bytes":%s,"diag_lines_round1":%s,"dart":"%s","os":"macOS %s","arch":"%s","chip":"%s","cores":%s,"ram_bytes":%s,"utc":"%s"}\n' \
    "$LABEL" "$CONTEXT" "$FILES" "$LOC" "$BYTES" "$ROUNDS" \
    "$COLD_MS" "$HOT_MIN" "$HOT_MEDIAN" "$RSS_MEDIAN" "$DIAGS1" \
    "$(json_escape "$DART_VER")" "$OS_VER" "$ARCH" "$(json_escape "$CHIP")" "$CORES" "$RAM_BYTES" "$STAMP"
# --cmd leg: the command text verbatim + its tool banner in place of "dart";
# every other field identical, so the mega A7 legs share one schema.
else
  printf '{"record":"summary","target":"%s","context":"%s","files":%s,"loc":%s,"bytes":%s,"rounds":%d,"cold_real_ms":%s,"hot_min_ms":%s,"hot_median_ms":%s,"maxrss_median_bytes":%s,"diag_lines_round1":%s,"cmd":"%s","tool_version":"%s","os":"macOS %s","arch":"%s","chip":"%s","cores":%s,"ram_bytes":%s,"utc":"%s"}\n' \
    "$LABEL" "$CONTEXT" "$FILES" "$LOC" "$BYTES" "$ROUNDS" \
    "$COLD_MS" "$HOT_MIN" "$HOT_MEDIAN" "$RSS_MEDIAN" "$DIAGS1" \
    "$(json_escape "$CMD")" "$(json_escape "$TOOL_VER")" "$OS_VER" "$ARCH" "$(json_escape "$CHIP")" "$CORES" "$RAM_BYTES" "$STAMP"
fi
