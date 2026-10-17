#!/usr/bin/env bash
# run-one.sh — run ONE fleet-probe instance against one prepared copy
# (ADR-multi-worktree-posture ticket T1).
#
# Wraps probe_one.py: spawns `dartq daemon <copy-dir>` in LSP mode, drives
# the full analysis pass (didOpen every *.dart file, waits for the complete
# publishDiagnostics wave), samples the daemon's process-tree RSS at 100 ms
# (analysis) / 1 s (idle) cadence, and emits exactly one JSON line
# (schema dartq-fleet-probe/1) on stdout. Human chatter goes to stderr.
#
# The daemon face leaves the L3 content store unarmed by default; the probe
# arms it explicitly at the standing 10 GiB default (--cache-size-limit
# 10GiB) so the machine-global sharing plane participates. --memory-limit
# is intentionally NOT set: the daemon's standing 2048 MiB budget is the
# posture the ADR's budget table assumes, and a ladder firing distorts the
# natural-peak measurement (it is flagged via the MemoryDegrade trace row).
#
# Env:
#   DARTQ_BIN        daemon binary (default <repo>/target/release/dartq)
#   DARTQ_DART_SDK   pinned Dart SDK root; falls back to $DART_SDK
#   XDG_CACHE_HOME   central L2/L3 root base — point it at a probe-local
#                    dir for isolation (see PLAN.md)
#
# Usage: run-one.sh [--dry-run] <copy-dir> [extra probe_one.py args]
#   Extra args pass through (e.g. --hold, --idle-seconds 30, --label a1).
#   --dry-run validates the arguments and environment without starting any
#   process (authoring-time verification mode).
set -euo pipefail

PROBE_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$PROBE_DIR/../../.." && pwd)"
BIN="${DARTQ_BIN:-$REPO_ROOT/target/release/dartq}"
SDK="${DARTQ_DART_SDK:-${DART_SDK:-}}"

DRY_RUN=0
PASSTHROUGH=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    *) PASSTHROUGH+=("$arg") ;;
  esac
done

if [ "${#PASSTHROUGH[@]}" -lt 1 ]; then
  echo "usage: run-one.sh [--dry-run] <copy-dir> [extra probe_one.py args]" >&2
  exit 2
fi
COPY_DIR="${PASSTHROUGH[0]}"

fail() { echo "run-one.sh: $*" >&2; exit 2; }

[ -x "$BIN" ] || fail "daemon binary not executable: $BIN (build with: cargo build --release -p dartq-cli)"
[ -d "$COPY_DIR" ] || fail "copy dir is not a directory: $COPY_DIR (run prepare.sh first)"
[ -f "$COPY_DIR/pubspec.yaml" ] || fail "no pubspec.yaml under $COPY_DIR"
[ -f "$COPY_DIR/.dart_tool/package_config.json" ] || fail "no .dart_tool/package_config.json under $COPY_DIR"
[ -n "$SDK" ] || fail "set DARTQ_DART_SDK (pinned Dart SDK root, >= 3.13) or DART_SDK"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "run-one.sh: dry-run OK"
  echo "  binary: $BIN"
  echo "  sdk:    $SDK"
  echo "  dir:    $COPY_DIR"
  echo "  central store: ${XDG_CACHE_HOME:-$HOME/.cache}/dartq"
  echo "  would run: probe_one.py $COPY_DIR --binary <BIN> --sdk <SDK> ${PASSTHROUGH[*]:1}"
  exit 0
fi

exec python3 "$PROBE_DIR/probe_one.py" "${PASSTHROUGH[@]}" \
  --binary "$BIN" --sdk "$SDK"
