#!/usr/bin/env bash
# plugin-ab.sh — plugin-on/off A-B performance protocol over the fixture
# presets (the #1089 plugin-overhead axis; arms and measurement points per
# tests/plugin-validation/README.md).
#
# Arms (identical lib/ corpus, single difference = the `plugins:` section):
#   A absent  presets/absent   — no plugin declarations, discovery short-circuit
#   B present presets/present  — all five vendored plugins declared
#
# Each arm: materialize lib/ from corpora, `dart pub get`, then one daemon
# session driven by tools/ab_driver.dart — cold full analysis (session
# launch, discovery, sidecar spawns, first full publish wave), N
# incremental rounds (per-round publish latency), graceful shutdown with
# per-plugin stats capture (peak_rss_bytes, busy_ms, degraded_cycles,
# restarts, evictions) plus a peak-RSS sampler over this lane's driver and
# daemon only (PID-scoped and identity-checked — never a machine-wide
# pattern sweep, which sums concurrent lanes' processes).
#
# A valid sample requires restarts=0 and degraded_cycles=0 on every plugin
# stats line (R7); invalid samples are still recorded and flagged.
#
# Serial-phase tool: builds dartq, runs one daemon instance at a time.
# Requires DARTQ_DART_SDK (pinned Dart SDK root, >= 3.13) and cargo.
# Usage: scripts/plugin-ab.sh [rounds] [idle_gap_seconds]
#   idle_gap_seconds > 0 adds an idle-gap phase to each arm: after the
#   incremental rounds the driver idles that long, then runs one post-gap
#   edit+publish round whose rounds_ms latency probes sidecar respawn +
#   re-analysis after idle eviction. Default 0 = off.
#   The daemon evicts an idle sidecar after idle_evict_ms (manifest default
#   300000; env override DARTQ_PLUGIN_IDLE_EVICT_MS, inherited by the
#   daemon): set that override below the gap when invoking, or the post-gap
#   round will measure a still-resident sidecar.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE="$REPO_ROOT/tests/plugin-validation"
ROUNDS="${1:-10}"
IDLE_GAP="${2:-0}"
case "$IDLE_GAP" in
  ''|*[!0-9]*)
    echo "plugin-ab: idle_gap_seconds must be a non-negative integer, got '$IDLE_GAP'" >&2
    exit 2
    ;;
esac

if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "plugin-ab: DARTQ_DART_SDK is required (pinned Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "plugin-ab: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "plugin-ab: cargo not found on PATH" >&2
  exit 2
fi

# One release build up front; the timed phases measure analysis, not compilation.
BIN="$REPO_ROOT/target/release/dartq"
if [ ! -x "$BIN" ]; then
  echo "plugin-ab: building dartq-cli (release)"
  cargo build --release -p dartq-cli --quiet
fi

# Materialize one arm's lib/ tree from the corpora, byte-identical across
# arms: flat corpora land under lib/<pkg>/, the clean_architecture_linter
# corpus keeps its feature-path layout (lib/features/...).
prepare() {
  local arm="$1" preset="$FIXTURE/presets/$arm" corpus name
  rm -rf "$preset/lib"
  mkdir -p "$preset/lib"
  for corpus in "$FIXTURE"/corpora/*/; do
    name="$(basename "$corpus")"
    if [ -d "$corpus/lib" ]; then
      cp -R "$corpus/lib/." "$preset/lib/"
    else
      mkdir -p "$preset/lib/$name"
      find "$corpus" -maxdepth 1 -name '*.dart' -exec cp {} "$preset/lib/$name/" \;
    fi
  done
  (cd "$preset" && dart pub get >/dev/null)
}

# Peak-RSS sampler over THIS lane's driver + daemon only, scoped by PID
# (concurrent lanes run byte-identical command lines, so the old
# machine-wide `ps -ax` marker sweep summed every lane's processes into
# each lane's sample). PIDs come from exact provenance: the driver from
# the script's own spawn ($!), the daemon from the driver's --pidfile
# record. Every sample re-checks the PID's current command line against
# its expected identity, so a dead-then-reused PID drops out instead of
# poisoning the peak. Plugin sidecars carry neither identity and were
# never summed — the plugin-present overhead number stays a lower bound.
# Writes the observed peak in KB to $2 when the marker file disappears.
sample_rss() {
  local marker="$1" out="$2" driver="$3" pid_file="$4" peak=0 total rss cmd daemon
  local bin
  bin="$(cat "$marker")"
  while [ -e "$marker" ]; do
    total=0
    if cmd="$(ps -o rss=,command= -p "$driver" 2>/dev/null)"; then
      case "$cmd" in
        *ab_driver.dart*) read -r rss _ <<<"$cmd"; total=$((total + rss)) ;;
      esac
    fi
    daemon=""
    [ -f "$pid_file" ] && daemon="$(cat "$pid_file" 2>/dev/null)"
    case "$daemon" in ''|*[!0-9]*) daemon="" ;; esac
    if [ -n "$daemon" ]; then
      if cmd="$(ps -o rss=,command= -p "$daemon" 2>/dev/null)"; then
        case "$cmd" in
          *"$bin daemon"*) read -r rss _ <<<"$cmd"; total=$((total + rss)) ;;
        esac
      fi
    fi
    if [ "$total" -gt "$peak" ]; then peak="$total"; fi
    sleep 0.2
  done
  echo "$peak" >"$out"
}

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RESULTS="$FIXTURE/results/$STAMP"
mkdir -p "$RESULTS"
echo "plugin-ab: rounds=$ROUNDS idle_gap=${IDLE_GAP}s results=${RESULTS#"$REPO_ROOT"/}"

# Optional idle-gap probe, passed through to the driver only when enabled
# (off by default — the baseline arm shape is unchanged).
idle_args=()
if [ "$IDLE_GAP" -gt 0 ]; then
  idle_args=(--idle-gap "$IDLE_GAP")
fi

# Drops the wedged plugin sidecars of THIS lane's daemon only: the daemon
# PID comes from the driver's --pidfile record and is identity-checked
# against the daemon's command line (a dead-then-reused PID never
# matches); only its live children carrying the sidecar entrypoint are
# killed, each by PID. Sidecars lead their own process groups by design —
# the daemon's group kill and the driver's SIGTERM never reach them and
# they have no signal handler — so this is the only drop they get. Never
# a command-pattern kill: pkill -f byte-matches any concurrent lane's
# sidecars (the cross-lane killer class behind the A-B incident).
kill_lane_sidecars() {
  local pid_file daemon pid
  pid_file="${daemon_pid_file:-}"
  [ -n "$pid_file" ] && [ -f "$pid_file" ] || return 0
  daemon="$(cat "$pid_file" 2>/dev/null)" || return 0
  case "$daemon" in ''|*[!0-9]*) return 0 ;; esac
  case "$(ps -o command= -p "$daemon" 2>/dev/null)" in
    *"$BIN daemon"*) ;;
    *) return 0 ;;
  esac
  for pid in $(pgrep -P "$daemon" 2>/dev/null); do
    case "$(ps -o command= -p "$pid" 2>/dev/null)" in
      *bin/dartq_serve.dart*) kill "$pid" 2>/dev/null || true ;;
    esac
  done
}

# On any exit path (set -e kill, SIGINT mid-arm) drop the active marker so
# the background sampler's loop terminates instead of orphaning, then drop
# this lane's wedged sidecars (kill_lane_sidecars above).
trap 'rm -f "${marker:-}"; kill_lane_sidecars' EXIT

for arm in absent present; do
  prepare "$arm"
  marker="$RESULTS/$arm.marker"
  daemon_pid_file="$RESULTS/$arm.daemon.pid"
  echo "$BIN" >"$marker"
  echo "plugin-ab: arm $arm — cold full + $ROUNDS incremental rounds${IDLE_GAP:+ + ${IDLE_GAP}s idle gap + 1 post-gap round}"
  # The driver runs backgrounded so its exact PID anchors the sampler; the
  # driver records the daemon's PID for the sampler and the exit trap
  # (--pidfile). The driver's JSON goes to the results file; daemon stderr
  # (stats lines included) is relayed into the log alongside the driver's
  # own output. `wait` preserves the real exit status (a negated `if !`
  # would report the negation's own 0).
  rm -f "$daemon_pid_file"
  status=0
  dart "$FIXTURE/tools/ab_driver.dart" \
    --daemon "$BIN" \
    --dir "$FIXTURE/presets/$arm" \
    --sdk "$DARTQ_DART_SDK" \
    --rounds "$ROUNDS" \
    ${idle_args[@]+"${idle_args[@]}"} \
    --pidfile "$daemon_pid_file" \
    >"$RESULTS/$arm.json" 2>"$RESULTS/$arm.log" &
  driver=$!
  sample_rss "$marker" "$RESULTS/$arm.rss" "$driver" "$daemon_pid_file" &
  sampler=$!
  wait "$driver" || status=$?
  if [ "$status" -ne 0 ]; then
    echo "plugin-ab: arm $arm driver failed (exit $status) — see $arm.log" >&2
  fi
  rm -f "$marker"
  wait "$sampler" || true

  # R7 sample-validity gate: flag restarts or drain degradation in the stats.
  if grep -qE 'restarts=[1-9]|degraded_cycles=[1-9]' "$RESULTS/$arm.log" 2>/dev/null; then
    echo "plugin-ab: arm $arm sample INVALID — restart/degradation in stats lines" \
      "(flagged in $arm.json.note)" | tee "$RESULTS/$arm.json.note"
  fi
  echo "plugin-ab: arm $arm done — cold_full_ms and round percentiles in $arm.json"
done

echo "plugin-ab: complete; aggregate $RESULTS/*.json (cold_full_ms delta B-A is the plugin-present overhead)"
