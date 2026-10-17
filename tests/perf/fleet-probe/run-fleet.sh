#!/usr/bin/env bash
# run-fleet.sh — run an N-instance fleet probe over prepared copies
# (ADR-multi-worktree-posture ticket T1).
#
# Starts N daemon instances (each via probe_one.py, in serial or parallel
# start mode), keeps every instance alive so the fleet is co-resident, and
# records:
#   - per-instance JSON lines (results/instance-<i>.json): baseline, peak,
#     steady, idle RSS + trace counts (content_id_hit warm hits)
#   - a co-residency timeline (results/fleet-samples.jsonl): summed subtree
#     RSS of all daemon roots, 1 s cadence
#   - the CENTRAL shared-store footprint: du of the machine-global L2/L3
#     root before/after the run plus per-subdir sizes. This measures ON-DISK
#     bytes — the cross-process dedup proof — not resident bytes. Under
#     today's posture (A1, all in-heap) RSS summation is honest private-page
#     accounting; once the T3 mmap CAS store lands, RSS double-counts shared
#     file-backed pages and the re-run must switch to per-process footprint
#     accounting (vmmap/footprint), counting file-backed pages once.
#
# Start modes:
#   serial (default)   — instance i+1 starts only after i emitted its line;
#                        i+1's cold pass reads the L2 generation i..i-1 wrote
#                        (cross-instance warm hits land in its traces). The
#                        co-resident plane measured after all N are up is the
#                        IDLE plane -> pins phi_idle. Co-resident PEAKS never
#                        overlap, so phi_peak needs the parallel mode.
#   parallel (--start parallel) — all N drivers launch at once and their
#                        analysis passes stack; the timeline's maximum is the
#                        synchronized peak plane -> pins phi_peak.
#
# Usage: run-fleet.sh [--dry-run] [--start serial|parallel] <N> <scale>
# Env: DARTQ_BIN, DARTQ_DART_SDK (or DART_SDK), DARTQ_FLEET_ROOT,
#   XDG_CACHE_HOME (probe-local root recommended), and the timing knobs
#   DARTQ_PROBE_IDLE_S (60), DARTQ_PROBE_DRIVER_TIMEOUT_S (7200),
#   DARTQ_PROBE_WAIT_TIMEOUT_S (7800), DARTQ_PROBE_HOLD_TIMEOUT_S (14400),
#   DARTQ_PROBE_COSETTLE_S (10), DARTQ_PROBE_FLEET_INTERVAL_S (1.0).
# Artifacts land in "$DARTQ_FLEET_ROOT/results/<stamp>-<scale>-n<N>/".
set -euo pipefail

PROBE_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$PROBE_DIR/../../.." && pwd)"
BIN="${DARTQ_BIN:-$REPO_ROOT/target/release/dartq}"
SDK="${DARTQ_DART_SDK:-${DART_SDK:-}}"
FLEET_ROOT="${DARTQ_FLEET_ROOT:-${TMPDIR:-/tmp}/dartq-fleet}"
CENTRAL="${XDG_CACHE_HOME:-$HOME/.cache}/dartq"

IDLE_S="${DARTQ_PROBE_IDLE_S:-60}"
DRIVER_TIMEOUT="${DARTQ_PROBE_DRIVER_TIMEOUT_S:-7200}"
WAIT_TIMEOUT="${DARTQ_PROBE_WAIT_TIMEOUT_S:-7800}"
HOLD_TIMEOUT="${DARTQ_PROBE_HOLD_TIMEOUT_S:-14400}"
COSETTLE_S="${DARTQ_PROBE_COSETTLE_S:-10}"
FLEET_INTERVAL="${DARTQ_PROBE_FLEET_INTERVAL_S:-1.0}"

START_MODE="serial"
DRY_RUN=0
ARGS=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --start) START_MODE="$2"; shift 2 ;;
    --start=*) START_MODE="${1#--start=}"; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done
case "$START_MODE" in serial|parallel) ;; *) echo "run-fleet.sh: --start must be serial or parallel, got '$START_MODE'" >&2; exit 2 ;; esac
if [ "${#ARGS[@]}" -ne 2 ]; then
  echo "usage: run-fleet.sh [--dry-run] [--start serial|parallel] <N> <scale:1k|10k|100k>" >&2
  exit 2
fi
N="${ARGS[0]}"
SCALE="${ARGS[1]}"
case "$N" in ''|*[!0-9]*) echo "run-fleet.sh: N must be a non-negative integer, got '$N'" >&2; exit 2 ;; esac
case "$SCALE" in 1k|10k|100k) ;; *) echo "run-fleet.sh: scale must be one of 1k, 10k, 100k, got '$SCALE'" >&2; exit 2 ;; esac
[ "$N" -ge 1 ] || { echo "run-fleet.sh: N must be >= 1" >&2; exit 2; }
[ -x "$BIN" ] || { echo "run-fleet.sh: daemon binary not executable: $BIN (build with: cargo build --release -p dartq-cli)" >&2; exit 2; }
[ -n "$SDK" ] || { echo "run-fleet.sh: set DARTQ_DART_SDK (pinned Dart SDK root, >= 3.13) or DART_SDK" >&2; exit 2; }

DEST_BASE="$FLEET_ROOT/$SCALE"
COPY_LIST=()
i=1
while [ "$i" -le "$N" ]; do
  COPY="$DEST_BASE/copy-$(printf '%03d' "$i")"
  if [ ! -f "$COPY/pubspec.yaml" ]; then
    echo "run-fleet.sh: missing copy $COPY — run: $PROBE_DIR/prepare.sh $SCALE $N" >&2
    exit 2
  fi
  COPY_LIST+=("$COPY")
  i=$(( i + 1 ))
done

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RESULTS="$FLEET_ROOT/results/$STAMP-$SCALE-n$N-$START_MODE"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "run-fleet.sh: dry-run OK"
  echo "  N=$N scale=$SCALE start=$START_MODE"
  echo "  binary: $BIN  sdk: $SDK"
  echo "  copies: $DEST_BASE/copy-001..$(printf '%03d' "$N")"
  echo "  results: $RESULTS"
  echo "  central store: $CENTRAL (du before/after + subdirs)"
  exit 0
fi

mkdir -p "$RESULTS"
PIDS="$RESULTS/pids.txt"
: > "$PIDS"

DU_BEFORE=0
if [ -d "$CENTRAL" ]; then
  DU_BEFORE="$(du -sk "$CENTRAL" | awk '{print $1}')"
fi
echo "run-fleet.sh: N=$N scale=$SCALE start=$START_MODE results=$RESULTS"
echo "run-fleet.sh: central store $CENTRAL before=${DU_BEFORE} KiB"

DRIVER_PIDS=()
DAEMON_PIDS=()
SAMPLER_PID=""

cleanup() {
  local pid deadline any_alive
  for pid in "${DRIVER_PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null || true
  done
  deadline=$(( SECONDS + 35 ))
  any_alive=1
  while [ "$any_alive" -eq 1 ] && [ "$SECONDS" -lt "$deadline" ]; do
    any_alive=0
    for pid in "${DRIVER_PIDS[@]:-}"; do
      [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && any_alive=1
    done
    [ "$any_alive" -eq 1 ] && sleep 1
  done
  for pid in "${DRIVER_PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -KILL "$pid" 2>/dev/null || true
  done
  for pid in "${DAEMON_PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -KILL "$pid" 2>/dev/null || true
  done
  [ -n "$SAMPLER_PID" ] && kill -TERM "$SAMPLER_PID" 2>/dev/null || true
}
trap cleanup EXIT

launch_instance() {
  local idx="$1" copy="$2"
  python3 "$PROBE_DIR/probe_one.py" "$copy" \
    --binary "$BIN" --sdk "$SDK" --label "instance-$idx" \
    --hold --hold-timeout "$HOLD_TIMEOUT" \
    --trace-events "$RESULTS/trace-$idx.jsonl" \
    --stderr-log "$RESULTS/daemon-$idx.log" \
    --idle-seconds "$IDLE_S" --timeout "$DRIVER_TIMEOUT" \
    >"$RESULTS/instance-$idx.json" 2>"$RESULTS/driver-$idx.log" &
  DRIVER_PIDS+=("$!")
}

await_instance() {
  local idx="$1"
  local file="$RESULTS/instance-$idx.json"
  local deadline=$(( SECONDS + WAIT_TIMEOUT ))
  local driver_pid="${DRIVER_PIDS[$(( idx - 1 ))]}"
  while [ ! -s "$file" ]; do
    if ! kill -0 "$driver_pid" 2>/dev/null; then
      echo "run-fleet.sh: instance $idx driver died before emitting its line — see $RESULTS/driver-$idx.log" >&2
      return 1
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "run-fleet.sh: instance $idx timed out waiting for its JSON line" >&2
      return 1
    fi
    sleep 2
  done
  local line
  line="$(head -n 1 "$file")"
  local daemon_pid valid
  daemon_pid="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("daemon_pid", 0))' "$line")"
  valid="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("valid", False))' "$line")"
  if [ "$valid" != "True" ]; then
    echo "run-fleet.sh: instance $idx emitted an INVALID sample: $line" >&2
    return 1
  fi
  echo "$daemon_pid" >> "$PIDS"
  DAEMON_PIDS+=("$daemon_pid")
  echo "run-fleet.sh: instance $idx up (daemon pid $daemon_pid)"
}

# The timeline sampler starts now: it re-reads the pids file each tick, so
# early ticks record the pre-fleet zero plane and instances register as they
# come up.
python3 "$PROBE_DIR/probe_one.py" fleet-sample \
  --pids-file "$PIDS" --out "$RESULTS/fleet-samples.jsonl" \
  --interval "$FLEET_INTERVAL" &
SAMPLER_PID=$!

instance_idx=1
if [ "$START_MODE" = "parallel" ]; then
  i=1
  while [ "$i" -le "$N" ]; do
    launch_instance "$i" "${COPY_LIST[$(( i - 1 ))]}"
    i=$(( i + 1 ))
  done
  i=1
  while [ "$i" -le "$N" ]; do
    await_instance "$i" || exit 3
    i=$(( i + 1 ))
  done
else
  while [ "$instance_idx" -le "$N" ]; do
    launch_instance "$instance_idx" "${COPY_LIST[$(( instance_idx - 1 ))]}"
    await_instance "$instance_idx" || exit 3
    instance_idx=$(( instance_idx + 1 ))
  done
fi

echo "run-fleet.sh: all $N instances up — co-resident settle ${COSETTLE_S}s"
sleep "$COSETTLE_S"
echo "STOP" >> "$PIDS"
kill -TERM "$SAMPLER_PID" 2>/dev/null || true
wait "$SAMPLER_PID" 2>/dev/null || true
SAMPLER_PID=""

python3 "$PROBE_DIR/probe_one.py" fleet-summarize \
  "$RESULTS/fleet-samples.jsonl" --tail-window 10 \
  > "$RESULTS/fleet-summary.json"

DU_AFTER=0
if [ -d "$CENTRAL" ]; then
  DU_AFTER="$(du -sk "$CENTRAL" | awk '{print $1}')"
fi
SUBDIRS_JSON="$(du -sk "$CENTRAL"/*/ 2>/dev/null | python3 -c '
import json, sys
out = {}
for line in sys.stdin:
    parts = line.split()
    if len(parts) == 2:
        out[parts[1].rstrip("/").split("/")[-1]] = int(parts[0])
print(json.dumps(out))' )"

INSTANCES_JSON="$(python3 -c '
import json, sys
print(json.dumps(["instance-%d.json" % i for i in range(1, int(sys.argv[1]) + 1)]))' "$N")"

DARTQ_FJ_SCALE="$SCALE" DARTQ_FJ_N="$N" DARTQ_FJ_START="$START_MODE" \
DARTQ_FJ_CENTRAL="$CENTRAL" DARTQ_FJ_BEFORE="$DU_BEFORE" DARTQ_FJ_AFTER="$DU_AFTER" \
DARTQ_FJ_SUBDIRS="$SUBDIRS_JSON" DARTQ_FJ_INSTANCES="$INSTANCES_JSON" \
DARTQ_FJ_SUMMARY="$(cat "$RESULTS/fleet-summary.json")" \
python3 - "$RESULTS" <<'PY'
import json, os, sys
results = sys.argv[1]
summary = json.loads(os.environ["DARTQ_FJ_SUMMARY"])
fleet = {
    "schema": "dartq-fleet-probe-fleet/1",
    "scale": os.environ["DARTQ_FJ_SCALE"],
    "N": int(os.environ["DARTQ_FJ_N"]),
    "start_mode": os.environ["DARTQ_FJ_START"],
    "results_dir": results,
    "central_store": {
        "path": os.environ["DARTQ_FJ_CENTRAL"],
        "du_kib_before": int(os.environ["DARTQ_FJ_BEFORE"]),
        "du_kib_after": int(os.environ["DARTQ_FJ_AFTER"]),
        "subdirs_kib": json.loads(os.environ.get("DARTQ_FJ_SUBDIRS", "{}")),
    },
    "fleet_rss_kib": summary["rss_kib"],
    "fleet_samples": summary["samples"],
    "tail_window_s": summary.get("tail_window_s", 10.0),
    "instance_files": json.loads(os.environ["DARTQ_FJ_INSTANCES"]),
    "measurement_caveats": [
        "central_store du is on-disk bytes (cross-process dedup proof), not resident bytes",
        "RSS summation is private-page accounting valid for today's all-in-heap posture (A1);",
        "after the T3 mmap CAS store lands, shared file-backed pages are double-counted by RSS",
        "and the re-run must use per-process footprint accounting (vmmap/footprint), counting",
        "file-backed pages once",
    ],
}
path = os.path.join(results, "fleet.json")
with open(path, "w", encoding="utf-8") as fh:
    json.dump(fleet, fh, indent=2)
    fh.write("\n")
print("run-fleet.sh: wrote", path)
PY

echo "run-fleet.sh: fleet RSS (KiB): $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["fleet_rss_kib"])' "$RESULTS/fleet-summary.json")"
echo "run-fleet.sh: central store after=${DU_AFTER} KiB (delta $(( DU_AFTER - DU_BEFORE )) KiB)"
echo "run-fleet.sh: shutting down the fleet"
for pid in "${DRIVER_PIDS[@]:-}"; do
  [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null || true
done
deadline=$(( SECONDS + 35 ))
any_alive=1
while [ "$any_alive" -eq 1 ] && [ "$SECONDS" -lt "$deadline" ]; do
  any_alive=0
  for pid in "${DRIVER_PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && any_alive=1
  done
  [ "$any_alive" -eq 1 ] && sleep 1
done
echo "run-fleet.sh: done — artifacts in $RESULTS"
echo "run-fleet.sh: next — analyze.py <this results dir> [more dirs...]"
