#!/usr/bin/env bash
# coverage-gate.sh — unit-test line-coverage gate for the dartq workspace.
#
# == Tool selection (recorded decision) ==
# cargo-llvm-cov (rustc-native -C instrument-coverage, source-based) over:
#   - cargo-tarpaulin: rejected. ptrace-based tracing has no macOS/arm64
#     support (this workspace's host) and is far slower than compiler
#     instrumentation.
#   - nextest coverage: not a distinct measurement tool — nextest only drives
#     cargo-llvm-cov underneath; adopting it adds a dependency, not data.
#   - cargo-llvm-cov: chosen. Exact line/region data from LLVM, actively
#     maintained, independent of the workspace rust-version (coverage is a
#     rustc/LLVM capability, not a crate-MSRV concern). Host prerequisites:
#     cargo-llvm-cov (cargo install cargo-llvm-cov --locked) and the
#     llvm-tools-preview component (rustup component add llvm-tools-preview);
#     a missing piece surfaces as exit 2 with the install hint, never as a
#     pass.
#
# == sccache interaction (documented caveat + mitigation) ==
# .cargo/config.toml wraps every rustc call with sccache (build.rustc-wrapper).
# Coverage builds inject -C instrument-coverage through RUSTFLAGS; if they
# flowed through the shared sccache cache, instrumented artifacts would be
# stored under coverage-shaped keys, and any key-semantics gap between sccache
# versions could hand a stale instrumented object to — or take entries from —
# normal builds: a cache-poisoning hazard in both directions. This script
# therefore runs its single cargo invocation with RUSTC_WRAPPER="" (cargo
# treats an empty env value as unsetting build.rustc-wrapper), bypassing
# sccache entirely: a coverage result can never depend on cache state, and
# the shared cache stays free of one-off instrumented artifacts. Accepted
# cost: every coverage run is a full cold rebuild — the gate lives in the
# serial phase, where a cold build is the normal posture anyway.
#
# SERIAL PHASE ONLY: this script executes the ENTIRE workspace test suite
# rebuilt with coverage instrumentation. Never run it alongside
# development-phase lanes (AGENTS.md memory-incident discipline); run it
# single-process with an RSS watchdog. `-- --test-threads=1` is forwarded to
# every test binary — no internal suite parallelism, and traces stay free of
# thread-scheduling nondeterminism. --dry-run prints the plan and runs
# nothing but the cargo-llvm-cov presence probe (the fail-closed tool gate).
#
# What a run does: one `cargo llvm-cov --workspace --tests` pass (lib unit
# tests + integration tests; benches are never built or run), then the lcov
# tracefile is folded into a per-crate table. Crate rows are derived from the
# tracefile itself — no hardcoded crate list; the plan below prints the
# workspace member list parsed from the root Cargo.toml at run time (ci.sh's
# idiom), while the actual measurement scope is --workspace (cargo's own
# authoritative member set). The gate fails (exit 1) when any crate row or
# the workspace total is below the floor. The floor is policy, not a
# placeholder: FLOOR_DEFAULT=0 keeps the gate advisory (the table prints, the
# gate never fails) until a measured baseline lands; raise it once numbers
# exist and the gate becomes enforcing.
#
# Stable outputs under target/coverage/:
#   coverage.lcov   lcov tracefile (the gate input)
#   summary.txt     toolchain banner + the per-crate table (rows sorted with
#                   LC_ALL=C — byte-stable across runs on one toolchain)
#   html/           browsable report (--html; llvm-cov emits one format per
#                   run, so --html replaces the lcov pass and evaluates no
#                   gate)
# A tracefile is only comparable within one toolchain: rustc and
# cargo-llvm-cov versions are stamped into every summary.txt.
#
# Usage:
#   scripts/coverage-gate.sh [--floor N] [--html] [--dry-run]
#   scripts/coverage-gate.sh --report FILE [--floor N]
#   scripts/coverage-gate.sh --help
#
#   --floor N      minimum acceptable line coverage percent (0-100), applied
#                  to every crate row and the workspace total. Default: 0
#                  (advisory) until a measured baseline exists.
#   --html         write the browsable HTML report under target/coverage/html
#                  instead of the lcov pass; prints the report pointer,
#                  evaluates no gate.
#   --dry-run      resolve every path, print the plan and the exact command;
#                  runs nothing but the cargo-llvm-cov presence probe.
#   --report FILE  re-evaluate the gate against an existing lcov tracefile
#                  instead of running the suite (no cargo-llvm-cov needed).
#   --output FILE  override the lcov destination (default:
#                  target/coverage/coverage.lcov).
#
# Exit codes: 0 = gate met (or advisory mode, dry run, help);
# 1 = below floor; 2 = usage error, missing tool, failed coverage run, or
# unreadable report.

set -euo pipefail

FLOOR_DEFAULT="0.0"
REPORT_DEFAULT="target/coverage/coverage.lcov"
HTML_DIR_DEFAULT="target/coverage/html"

usage() {
  sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

FLOOR="$FLOOR_DEFAULT"
REPORT="$REPORT_DEFAULT"
HTML_DIR="$HTML_DIR_DEFAULT"
MODE="run" # run (lcov + gate) | html (browsable, no gate) | report (re-gate)
DRY_RUN=0

while [ $# -gt 0 ]; do
  # Termination invariant: every arm consumes its tokens (>=1 per iteration)
  # or exits — zero-value arms shift 1, value arms shift 2 behind the
  # [ $# -ge 2 ] guard. No arm may fall through without consuming.
  case "$1" in
    --floor)
      [ $# -ge 2 ] || { echo "coverage-gate: --floor needs a value" >&2; exit 2; }
      FLOOR="$2"; shift 2 ;;
    --output)
      [ $# -ge 2 ] || { echo "coverage-gate: --output needs a value" >&2; exit 2; }
      [ "$MODE" = "run" ] || { echo "coverage-gate: --output only applies to the lcov run mode" >&2; exit 2; }
      REPORT="$2"; shift 2 ;;
    --html)
      [ "$MODE" = "run" ] || { echo "coverage-gate: --html cannot be combined with --report" >&2; exit 2; }
      MODE="html"; shift ;;
    --report)
      [ $# -ge 2 ] || { echo "coverage-gate: --report needs a value" >&2; exit 2; }
      [ "$MODE" = "run" ] || { echo "coverage-gate: --report cannot be combined with --html" >&2; exit 2; }
      MODE="report"; REPORT="$2"; shift 2 ;;
    --dry-run)
      [ "$MODE" != "report" ] || { echo "coverage-gate: --dry-run does not apply to --report mode" >&2; exit 2; }
      DRY_RUN=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "coverage-gate: unknown argument: $1 (see --help)" >&2; usage >&2; exit 2 ;;
  esac
done

case "$FLOOR" in
  ''|*[!0-9.]*|*.*.*) echo "coverage-gate: --floor must be a number in 0-100, got: $FLOOR" >&2; exit 2 ;;
esac
awk -v f="$FLOOR" 'BEGIN { exit !(f >= 0 && f <= 100) }' \
  || { echo "coverage-gate: --floor must be within 0-100, got: $FLOOR" >&2; exit 2; }

# Repo root: everything below runs relative to the workspace.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"
[ -f Cargo.toml ] || { echo "coverage-gate: no workspace Cargo.toml at $ROOT" >&2; exit 2; }

banner=""
if [ "$MODE" != "report" ]; then
  # Plan + preflight (run and html modes). Member list parsed from the root
  # Cargo.toml at run time — the ci.sh idiom, never a hardcoded list.
  members="$(sed -n '/^members = \[/,/^\]/p' Cargo.toml | grep -o '"[^"]*"' | tr -d '"')" || members=""
  [ -n "$members" ] || { echo "coverage-gate: no workspace members parsed from Cargo.toml" >&2; exit 2; }

  command -v cargo >/dev/null 2>&1 \
    || { echo "coverage-gate: cargo not on PATH; the coverage gate cannot run" >&2; exit 2; }
  # The gate fails closed: a missing cargo-llvm-cov is a red gate, never a pass.
  if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "coverage-gate: cargo-llvm-cov is not installed; the coverage gate cannot run." >&2
    echo "  install once: cargo install cargo-llvm-cov --locked" >&2
    echo "  also required: rustup component add llvm-tools-preview" >&2
    exit 2
  fi

  if [ "$FLOOR" = "0" ] || [ "$FLOOR" = "0.0" ]; then
    floor_note="advisory (table prints, gate never fails)"
  else
    floor_note="enforcing"
  fi
  echo "== coverage-gate plan"
  printf '  %-14s %s\n' "floor" "$FLOOR  ($floor_note)"
  printf '  %-14s %s crates: %s\n' "members" "$(printf '%s\n' $members | wc -l | tr -d ' ')" "$(printf '%s ' $members)"
  if [ "$MODE" = "html" ]; then
    printf '  %-14s %s\n' "output" "$HTML_DIR/ (browsable; no lcov, no gate)"
    printf '  %-14s %s\n' "command" "RUSTC_WRAPPER='' cargo llvm-cov --workspace --tests --offline --html --output-dir $HTML_DIR -- --test-threads=1"
  else
    printf '  %-14s %s\n' "output" "$REPORT (+ summary.txt alongside)"
    printf '  %-14s %s\n' "command" "RUSTC_WRAPPER='' cargo llvm-cov --workspace --tests --offline --lcov --output-path $REPORT -- --test-threads=1"
  fi
  printf '  %-14s %s\n' "suite" "cargo llvm-cov --workspace --tests (lib + integration tests; benches excluded)"
  printf '  %-14s %s\n' "threads" "-- --test-threads=1 (serial, deterministic traces)"
  printf '  %-14s %s\n' "wrapper" "RUSTC_WRAPPER='' for this invocation (sccache bypassed — see header)"

  if [ "$DRY_RUN" = 1 ]; then
    echo "== dry run: plan resolved, nothing executed"
    exit 0
  fi

  # Serial execution: one workspace pass, test binaries pinned to a single
  # thread via the forwarded arg (see the serial-phase note in the header).
  # RUSTC_WRAPPER="" unsets the workspace's sccache wrapper for this
  # invocation only (see the sccache section in the header).
  rc=0
  if [ "$MODE" = "html" ]; then
    mkdir -p "$HTML_DIR"
    RUSTC_WRAPPER="" cargo llvm-cov --workspace --tests --offline --html --output-dir "$HTML_DIR" -- --test-threads=1 || rc=$?
  else
    mkdir -p "$(dirname "$REPORT")"
    RUSTC_WRAPPER="" cargo llvm-cov --workspace --tests --offline --lcov --output-path "$REPORT" -- --test-threads=1 || rc=$?
  fi
  if [ "$rc" -ne 0 ]; then
    echo "coverage-gate: coverage run FAILED (exit $rc); if the llvm tools are missing: rustup component add llvm-tools-preview" >&2
    exit 2
  fi

  banner="$(rustc --version) / $(cargo llvm-cov --version)"
fi

if [ "$MODE" = "html" ]; then
  echo "coverage-gate: HTML report ready at $HTML_DIR/index.html"
  echo "coverage-gate: no lcov trace was produced by --html; rerun without --html to evaluate the gate"
  exit 0
fi

[ -f "$REPORT" ] || { echo "coverage-gate: coverage report not found: $REPORT" >&2; exit 2; }

# Fold the tracefile into per-crate (lines-hit, lines-total) pairs. SF paths
# are matched by substring so absolute and relative traces both parse.
ROWS="$(awk '
  /^SF:/ {
    path = substr($0, 5)
    crate = "other"
    if (match(path, "crates/[^/]+/")) {
      crate = substr(path, RSTART + 7, RLENGTH - 8)
    } else if (index(path, "tests/harness/") != 0) {
      crate = "harness"
    }
    cur = crate
  }
  /^LH:/ { lh[cur] += substr($0, 4) }
  /^LF:/ { lf[cur] += substr($0, 4) }
  END { for (c in lf) if (lf[c] > 0) printf "%s %d %d\n", c, lh[c], lf[c] }
' "$REPORT" | LC_ALL=C sort)"

[ -n "$ROWS" ] || { echo "coverage-gate: no coverage records parsed from $REPORT (not an lcov tracefile?)" >&2; exit 2; }

# Print the summary table and enforce the floor on every row and the total.
# Violations are listed on stderr; the awk exit code becomes the gate result.
set +e
TABLE="$(printf '%s\n' "$ROWS" | awk -v floor="$FLOOR" '
  {
    hit[$1] = $2; total[$1] = $3
    order[++n] = $1
    all_hit += $2; all_total += $3
  }
  function pct(h, t) { return (t > 0) ? h * 100 / t : 100 }
  END {
    printf "%-24s %10s %12s %9s\n", "crate", "lines-hit", "lines-total", "coverage"
    for (i = 1; i <= n; i++) {
      c = order[i]
      printf "%-24s %10d %12d %8.1f%%\n", c, hit[c], total[c], pct(hit[c], total[c])
      if (pct(hit[c], total[c]) < floor) bad = bad "  " c " (" sprintf("%.1f", pct(hit[c], total[c])) "%)"
    }
    printf "%-24s %10d %12d %8.1f%%\n", "TOTAL", all_hit, all_total, pct(all_hit, all_total)
    printf "floor: %.1f%%\n", floor
    if (pct(all_hit, all_total) < floor) bad = bad "  TOTAL (" sprintf("%.1f", pct(all_hit, all_total)) "%)"
    if (bad != "") {
      printf "coverage gate FAILED below floor %.1f%%:\n%s\n", floor, bad > "/dev/stderr"
      exit 1
    }
    printf "coverage gate PASSED (floor %.1f%%)\n", floor
  }
')"
GATE_RC=$?
set -e

printf '%s\n' "$TABLE"

# Persist the record next to the tracefile: toolchain banner (empty in
# --report mode, where the trace's producing toolchain is unknown) + the
# table. LC_ALL=C row ordering keeps this byte-stable run over run.
{
  [ -z "$banner" ] || printf '%s\n' "$banner"
  printf '%s\n' "$TABLE"
} > "$(dirname "$REPORT")/summary.txt"

exit "$GATE_RC"
