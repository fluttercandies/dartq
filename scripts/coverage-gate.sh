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
# == target-dir isolation ==
# The whole invocation builds into CARGO_TARGET_DIR=target/coverage-build,
# never the shared target/: a coverage run compiles instrumented binaries,
# and letting them land in the shared target/debug/ would overwrite the
# exact dartq/harness binaries scripts/m2-gate.sh consumes as its gate
# inputs (instrumented binaries also drop .profraw files into their cwd on
# every run). The isolated tree sits inside the already-gitignored target/,
# so no ignore-file change; the cost is a second cold build tree per run,
# consistent with the sccache-bypassed cold-build posture above.
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
# authoritative member set). Each crate row is gated against its own floor
# from the per-crate policy table scripts/coverage-floors.tsv (tier
# gradient: pure-logic crates 80, semantic/engine 65, I/O/CLI boundary 50;
# `default` fallback 0; rationale and ratchet discipline live inside the
# table and docs/plans/coverage-gate.md section 6). --floor N overrides the table
# with one global floor. TOTAL is printed but always advisory — a global
# percentage is gameable by stub growth and mixes I/O crates with pure
# logic. Default posture is advisory: below-floor rows are reported, exit
# stays 0; --enforce flips the gate to exit 1 once the measured baseline is
# committed (numbers exist before any floor bites).
#
# Stable outputs under target/coverage/:
#   coverage.lcov   lcov tracefile (the gate input)
#   summary.txt     toolchain banner + git identity stamp + the per-crate
#                   table (LC_ALL=C sort and decimal point — byte-stable)
#   tuple.json      the §19.5 identity tuple sealed over summary.txt
#   html/           browsable report (--html; llvm-cov emits one format per
#                   run, so --html replaces the lcov pass and evaluates no
#                   gate)
# §19.5 seal: a run-mode summary.txt carries `git_head` and
# `worktree_patch_sha256` lines (via scripts/repro-tuple.sh — the single
# source of the identity computation, so the stamp and tuple.json agree
# field for field; a clean tree hashes the empty diff). Gate
# reproducibility contract: same committed HEAD + same worktree patch +
# same toolchain ⇒ byte-identical summary.txt. For CI-grade comparisons
# run from a clean committed tree (untracked material is not part of the
# identity).
#
# Usage:
#   scripts/coverage-gate.sh [--floors FILE | --floor N] [--enforce] [--html] [--dry-run]
#   scripts/coverage-gate.sh --report FILE [--floors FILE | --floor N] [--enforce]
#   scripts/coverage-gate.sh --help
#
#   --floors FILE  per-crate floor policy table: crate<TAB>percent rows,
#                  '#' comments, one `default` fallback row (see
#                  scripts/coverage-floors.tsv). Default:
#                  scripts/coverage-floors.tsv. Each crate row is gated
#                  against its own floor; the table is validated before any
#                  coverage work. Mutually exclusive with --floor.
#   --floor N      override the table with one global floor percent (0-100)
#                  on every crate row (TOTAL stays advisory). Mutually
#                  exclusive with --floors.
#   --enforce      fail (exit 1) when a crate row is below its floor.
#                  Default: advisory — below-floor rows are reported on
#                  stderr, exit stays 0; flip once the measured baseline is
#                  committed (docs/plans/coverage-gate.md section 6).
#   --html         write the browsable HTML report under target/coverage/html
#                  instead of the lcov pass; prints the report pointer,
#                  evaluates no gate.
#   --dry-run      resolve every path, validate the floors table, print the
#                  plan and the exact command; runs nothing but the
#                  cargo-llvm-cov presence probe.
#   --report FILE  re-evaluate the gate against an existing lcov tracefile
#                  instead of running the suite (no cargo-llvm-cov needed;
#                  the summary written in this mode carries no git stamp —
#                  the trace's producing environment is unknown).
#   --output FILE  override the lcov destination (default:
#                  target/coverage/coverage.lcov).
#
# Exit codes: 0 = gate met (or advisory mode, dry run, help);
# 1 = enforcing gate red (a crate row below its floor); 2 = usage error,
# invalid floors table, missing tool, failed coverage run, or unreadable
# report.

set -euo pipefail

# Coverage builds are isolated from the shared target/ (rationale in the
# target-dir isolation section of the header): instrumented binaries must
# never leak into the gate-consumed target/debug face. The relative path
# resolves under the workspace root — the script cd's there before any
# cargo call.
export CARGO_TARGET_DIR="target/coverage-build"

FLOOR_DEFAULT="0.0"
REPORT_DEFAULT="target/coverage/coverage.lcov"
HTML_DIR_DEFAULT="target/coverage/html"

usage() {
  sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

FLOOR="$FLOOR_DEFAULT"
REPORT="$REPORT_DEFAULT"
HTML_DIR="$HTML_DIR_DEFAULT"
FLOORS=""
MODE="run" # run (lcov + gate) | html (browsable, no gate) | report (re-gate)
DRY_RUN=0
ENFORCE=0
FLOOR_EXPLICIT=0

while [ $# -gt 0 ]; do
  # Termination invariant: every arm consumes its tokens (>=1 per iteration)
  # or exits — zero-value arms shift 1, value arms shift 2 behind the
  # [ $# -ge 2 ] guard. No arm may fall through without consuming.
  case "$1" in
    --floor)
      [ $# -ge 2 ] || { echo "coverage-gate: --floor needs a value" >&2; exit 2; }
      FLOOR="$2"; FLOOR_EXPLICIT=1; shift 2 ;;
    --floors)
      [ $# -ge 2 ] || { echo "coverage-gate: --floors needs a value" >&2; exit 2; }
      [ "$MODE" != "html" ] || { echo "coverage-gate: --floors does not apply to --html (no gate evaluated)" >&2; exit 2; }
      FLOORS="$2"; shift 2 ;;
    --enforce)
      [ "$MODE" != "html" ] || { echo "coverage-gate: --enforce does not apply to --html (no gate evaluated)" >&2; exit 2; }
      ENFORCE=1; shift ;;
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
if [ "$FLOOR_EXPLICIT" = 1 ] && [ -n "$FLOORS" ]; then
  echo "coverage-gate: --floor and --floors are mutually exclusive (one policy source at a time)" >&2
  exit 2
fi

# Repo root: everything below runs relative to the workspace.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"
[ -f Cargo.toml ] || { echo "coverage-gate: no workspace Cargo.toml at $ROOT" >&2; exit 2; }

# Per-crate floor policy: table by default, global --floor override. The
# parse block below is shared verbatim by the preflight validator and the
# gate awk (one definition, so both always agree on what a valid table is);
# the validator runs before any coverage work so a typo'd policy fails the
# plan, not the post-run gate.
FLOORS_PARSE='
NR==FNR && gfloor < 0 {
  if ($0 == "" || $0 ~ /^#/) next
  if (split($0, f, "\t") != 2 || f[1] == "" || f[2] !~ /^[0-9]+(\.[0-9]+)?$/ || f[2] + 0 > 100) {
    printf "coverage-gate: malformed floors table row %d: %s\n", FNR, $0 > "/dev/stderr"
    parse_err = 1; exit
  }
  if (f[1] == "default") {
    if (seen_default++) { printf "coverage-gate: duplicate default row in floors table\n" > "/dev/stderr"; parse_err = 1; exit }
    dflt = f[2] + 0; next
  }
  if (f[1] in fl) { printf "coverage-gate: duplicate floor row: %s\n", f[1] > "/dev/stderr"; parse_err = 1; exit }
  fl[f[1]] = f[2] + 0
  next
}
'
resolve_floors() {
  if [ "$MODE" = "html" ]; then return 0; fi
  if [ "$FLOOR_EXPLICIT" = 1 ]; then return 0; fi
  FLOORS="${FLOORS:-$SCRIPT_DIR/coverage-floors.tsv}"
  [ -f "$FLOORS" ] || { echo "coverage-gate: floors table not found: $FLOORS" >&2; exit 2; }
  LC_ALL=C awk -v gfloor="-1" "${FLOORS_PARSE}"' END {
    if (parse_err) exit 2
    if (NR == 0 || seen_default != 1) {
      printf "coverage-gate: floors table is empty or has no `default` row: %s\n", FILENAME > "/dev/stderr"
      exit 2
    }
  }' "$FLOORS" || { echo "coverage-gate: invalid floors table: $FLOORS" >&2; exit 2; }
}

banner=""
GIT_HEAD=""
PATCH_SHA=""
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

  resolve_floors
  if [ "$ENFORCE" = 1 ]; then
    gate_note="enforcing (below-floor crate row fails, exit 1)"
  else
    gate_note="advisory (below-floor rows reported, exit 0; flip with --enforce)"
  fi
  echo "== coverage-gate plan"
  if [ "$FLOOR_EXPLICIT" = 1 ]; then
    printf '  %-14s %s\n' "floors" "global override $FLOOR (TOTAL advisory)"
  else
    printf '  %-14s %s\n' "floors" "$FLOORS (TOTAL advisory)"
  fi
  printf '  %-14s %s\n' "gate" "$gate_note"
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
  printf '  %-14s %s\n' "target-dir" "target/coverage-build (isolated from the shared target/ — see header)"

  if [ "$DRY_RUN" = 1 ]; then
    echo "== dry run: plan resolved, nothing executed"
    exit 0
  fi

  # Serial execution: one workspace pass, test binaries pinned to a single
  # thread via the forwarded arg (see the serial-phase note in the header).
  # RUSTC_WRAPPER="" unsets the workspace's sccache wrapper for this
  # invocation only (see the sccache section in the header). --offline
  # assumes a dev host with a warm cargo registry: on a machine with no
  # populated registry the run fails (accepted — serial-phase,
  # dev-host-only tooling).
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

  # §19.5 seal (docs/plans/coverage-gate.md section 5): stamp the run's
  # environment identity from repro-tuple.sh — the single source of the
  # identity computation, so the summary stamp and tuple.json agree field
  # for field. A clean tree hashes the empty diff (sha256 of zero bytes),
  # the same value the tuple records.
  tuple="$("$SCRIPT_DIR/repro-tuple.sh")"
  GIT_HEAD="$(printf '%s\n' "$tuple" | sed -n 's/.*"git_head":"\([0-9a-f]*\)".*/\1/p')"
  PATCH_SHA="$(printf '%s\n' "$tuple" | sed -n 's/.*"worktree_patch_sha256":"\([0-9a-f]*\)".*/\1/p')"
  [ -n "$GIT_HEAD" ] && [ -n "$PATCH_SHA" ] \
    || { echo "coverage-gate: could not read the identity fields from repro-tuple.sh output" >&2; exit 2; }
fi

if [ "$MODE" = "html" ]; then
  echo "coverage-gate: HTML report ready at $HTML_DIR/index.html"
  echo "coverage-gate: no lcov trace was produced by --html; rerun without --html to evaluate the gate"
  exit 0
fi

[ -f "$REPORT" ] || { echo "coverage-gate: coverage report not found: $REPORT" >&2; exit 2; }
resolve_floors

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

# Print the summary table and evaluate each crate row against its floor
# (per-crate table, or the global --floor override). TOTAL is printed but
# never gated. Advisory mode reports below-floor rows and exits 0; --enforce
# turns them into exit 1. Violations are listed on stderr; the awk exit code
# becomes the gate result. LC_ALL=C pins the sort and the decimal point so
# summaries stay byte-stable.
set +e
# Operands: the floors table first (NR==FNR), then `-` — without a `-`
# operand awk would stop after the file and never read the row stream.
# Both arrays are non-empty: an empty "${arr[@]}" trips `set -u` on bash 3.2.
if [ "$FLOOR_EXPLICIT" = 1 ]; then
  GATE_AWK_ARGS=(-v gfloor="$FLOOR" -v enforce="$ENFORCE")
  GATE_AWK_FILES=(-)
else
  GATE_AWK_ARGS=(-v gfloor="-1" -v enforce="$ENFORCE")
  GATE_AWK_FILES=("$FLOORS" -)
fi
TABLE="$(printf '%s\n' "$ROWS" | LC_ALL=C awk "${GATE_AWK_ARGS[@]}" "${FLOORS_PARSE}"'
  {
    hit[$1] = $2; total[$1] = $3
    order[++n] = $1
    all_hit += $2; all_total += $3
  }
  function pct(h, t) { return (t > 0) ? h * 100 / t : 100 }
  END {
    if (parse_err) exit 2
    printf "%-24s %10s %12s %9s %6s\n", "crate", "lines-hit", "lines-total", "coverage", "floor"
    for (i = 1; i <= n; i++) {
      c = order[i]
      p = pct(hit[c], total[c])
      row_floor = (gfloor >= 0) ? gfloor : ((c in fl) ? fl[c] : dflt)
      printf "%-24s %10d %12d %8.1f%% %6.1f\n", c, hit[c], total[c], p, row_floor
      if (p < row_floor) {
        bad = bad "  " c " (" sprintf("%.1f", p) "% < floor " sprintf("%.1f", row_floor) "%)"
        nbad++
      }
    }
    printf "%-24s %10d %12d %8.1f%% %6s\n", "TOTAL", all_hit, all_total, pct(all_hit, all_total), "-"
    if (gfloor >= 0) printf "floor: %.1f%% (global override; TOTAL advisory)\n", gfloor
    else printf "floors: per-crate policy table (TOTAL advisory)\n"
    if (nbad > 0) {
      if (enforce) {
        printf "coverage gate FAILED: %d crate row(s) below floor (--enforce):\n%s\n", nbad, bad > "/dev/stderr"
        exit 1
      }
      printf "coverage gate ADVISORY: %d crate row(s) below floor (rerun with --enforce to fail):\n%s\n", nbad, bad
    } else {
      printf "coverage gate PASSED\n"
    }
  }
' "${GATE_AWK_FILES[@]}")"
GATE_RC=$?
set -e

printf '%s\n' "$TABLE"

# Persist the record next to the tracefile: toolchain banner + §19.5 git
# identity stamp (run mode only — in --report mode the trace's producing
# environment is unknown, so neither is stamped) + the table. LC_ALL=C row
# ordering and the pinned decimal point keep this byte-stable run over run:
# same committed HEAD + same worktree patch + same toolchain ⇒ byte-identical
# summary.txt (docs/plans/coverage-gate.md section 5).
{
  [ -z "$banner" ] || printf '%s\n' "$banner"
  if [ -n "$GIT_HEAD" ]; then
    printf 'git_head %s\n' "$GIT_HEAD"
    printf 'worktree_patch_sha256 %s\n' "$PATCH_SHA"
  fi
  printf '%s\n' "$TABLE"
} > "$(dirname "$REPORT")/summary.txt"

# Seal the §19.5 tuple over the summary so the record and its identity ship
# together; the CI assertion (when workflows exist) compares both against
# the previous baseline.
if [ -n "$GIT_HEAD" ]; then
  "$SCRIPT_DIR/repro-tuple.sh" --output "$(dirname "$REPORT")/summary.txt" \
    > "$(dirname "$REPORT")/tuple.json"
fi

exit "$GATE_RC"
