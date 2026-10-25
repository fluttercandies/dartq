#!/usr/bin/env bash
# run.sh — the dartq process-level e2e suite (#2586): every CLI verb
# under complex real-world situations, against the real release binary
# over real package trees.
#
# Usage:
#   cargo build --release -p dartq-cli
#   tests/e2e/run.sh
#
# Prerequisites: bash, python3, git (the worktree legs). The real Dart
# SDK (default the fvm default, override with DARTQ_DART_SDK) is needed
# only by the dart-toolchain legs (the test-run handle, the SDK-symbol
# rename refusal, the pub-get cross-situation); without it those legs
# report SKIP and the rest of the suite still runs. Evidence lands under
# target/evidence/e2e/ (per-leg stdout/stderr/rc plus this run's legs
# table); the scratch trees and the isolated session registry are removed
# at exit and every spawned process is torn down (the zero-orphan rule).
#
# Serial-phase tool: one dartq process per leg (plus its resident session
# where the leg tests one), no cargo, no oracle.

# No `set -e`: legs collect failures through the harness and keep going.
set -uo pipefail

cd "$(dirname "$0")"
source lib/common.sh

mkdir -p "$EVIDENCE"

if [ ! -x "$DARTQ" ]; then
  echo "e2e: dartq binary not found at $DARTQ (cargo build --release -p dartq-cli)" >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "e2e: python3 is required" >&2
  exit 2
fi
if ! command -v git >/dev/null 2>&1; then
  echo "e2e: git is required (the worktree legs)" >&2
  exit 2
fi

echo "dartq e2e suite — binary $DARTQ"
echo "scratch root $E2E_WORK (isolated caches), evidence $EVIDENCE"
echo

# Each leg family sources in and runs; leg_begin/ok/bad above print the
# live verdict stream and the trap aggregates at exit.
source legs/01-packages.sh
source legs/02-session.sh
source legs/03-events.sh
source legs/04-resolve-complete.sh
source legs/05-compat.sh
source legs/06-fix-rename.sh
source legs/07-mcp.sh
source legs/08-cross.sh
source legs/09-exit-codes.sh

legs_01_run
legs_02_run
legs_03_run
legs_04_run
legs_05_run
legs_06_run
legs_07_run
legs_08_run
legs_09_run

# Zero-orphan gate: nothing the suite spawned may outlive it — any live
# dartq process whose command line OR working directory belongs to this
# run's scratch root fails the leg. Never the user's own dartq sessions
# or MCP hosts (they live elsewhere). The cleanup trap (which runs
# session stop --all under the isolated registry) fires after this
# check, so a leak here fails the suite while the trap still sweeps.
leg_begin zero-orphans "no dartq/dart/flutter process outlives the suite"
"$DARTQ" session stop --all >/dev/null 2>&1 || true
sleep 0.3
orphans="$(ps -axo pid=,command= | grep -E '[d]artq' | awk -v work="$E2E_WORK" '
  $0 ~ work { print; next }
  {
    # A child spawned with cwd inside the scratch root (e.g. a dartq
    # test runner) carries the root in its cwd, not its command line.
    pid = $1
    cmd = "lsof -a -p " pid " -d cwd -Fn 2>/dev/null"
    while ((cmd | getline line) > 0)
      if (index(line, work) == 2) { print; break }
    close(cmd)
  }')"
if [ -n "$orphans" ]; then
  bad "no spawned process survives" "$orphans"
else
  ok "no spawned process survives (scoped to $E2E_WORK)"
fi

printf '\n suite complete — see target/evidence/e2e/\n'
