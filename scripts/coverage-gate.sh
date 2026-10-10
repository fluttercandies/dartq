#!/usr/bin/env bash
# coverage-gate.sh — unit-test coverage gate for the dartq workspace.
#
# Runs `cargo llvm-cov` over every workspace test target (--tests: lib unit
# tests + integration tests; benches are never built or run), parses the
# resulting lcov tracefile into a per-crate summary table, and fails
# (exit 1) when any crate row or the workspace total falls below the
# coverage floor. --report <tracefile> re-evaluates the gate against an
# existing tracefile without re-running the suite (no cargo-llvm-cov
# needed on that path).
#
# Resource discipline (AGENTS.md): this script executes the entire workspace
# test suite. Run it ONLY in the serial verification phase, single-process,
# with an RSS watchdog; `-- --test-threads=1` is forwarded to every test
# binary so no suite parallelizes internally. Never run two coverage
# invocations concurrently.
#
# The floor is policy, set deliberately — not a placeholder to be ignored.
# While llvm-cov numbers have not yet landed the default is 0 (advisory
# mode: the table prints, the gate never fails); raise FLOOR_DEFAULT once a
# measured baseline exists and the release gate becomes enforcing.
FLOOR_DEFAULT=0.0

# Coverage report destination when running the suite (re-gating an existing
# report needs --report instead).
REPORT_DEFAULT="target/coverage.lcov"

usage() {
  cat <<EOF
usage: scripts/coverage-gate.sh [--floor N] [--output FILE] | [--report FILE]

  --floor N     Minimum acceptable line coverage percentage (0-100). Applies
                to every crate row and to the workspace total; default is
                FLOOR_DEFAULT (${FLOOR_DEFAULT}) from this script's header.
  --output FILE Where to write the lcov tracefile (default: ${REPORT_DEFAULT}).
  --report FILE Evaluate the gate against an existing lcov tracefile instead
                of running the suite (no cargo-llvm-cov required).

Exit codes: 0 = gate passed (or advisory mode); 1 = below floor / gate red;
2 = usage error, missing tool, or unreadable report.
EOF
}

set -euo pipefail

FLOOR="$FLOOR_DEFAULT"
REPORT="$REPORT_DEFAULT"
MODE="run"

while [ $# -gt 0 ]; do
  case "$1" in
    --floor)
      [ $# -ge 2 ] || { echo "error: --floor needs a value" >&2; exit 2; }
      FLOOR="$2"; shift 2 ;;
    --output)
      [ $# -ge 2 ] || { echo "error: --output needs a value" >&2; exit 2; }
      REPORT="$2"; shift 2 ;;
    --report)
      [ $# -ge 2 ] || { echo "error: --report needs a value" >&2; exit 2; }
      MODE="report"; REPORT="$2"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$FLOOR" in
  ''|*[!0-9.]*) echo "error: --floor must be numeric, got: $FLOOR" >&2; exit 2 ;;
esac
awk -v f="$FLOOR" 'BEGIN { exit !(f >= 0 && f <= 100) }' \
  || { echo "error: --floor must be within 0-100, got: $FLOOR" >&2; exit 2; }

# Repo root: everything below runs relative to the workspace.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"
[ -f Cargo.toml ] || { echo "error: no workspace Cargo.toml at $ROOT" >&2; exit 2; }

if [ "$MODE" = "run" ]; then
  command -v cargo >/dev/null 2>&1 \
    || { echo "error: cargo not on PATH; the coverage gate cannot run" >&2; exit 2; }
  # The gate fails closed: a missing cargo-llvm-cov is a red gate, never a pass.
  if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "error: cargo-llvm-cov is not installed; the coverage gate cannot run." >&2
    echo "       install it once: cargo install cargo-llvm-cov --locked" >&2
    exit 2
  fi
  # Serial execution: one workspace pass, test binaries pinned to a single
  # thread via the forwarded arg (see the resource-discipline note above).
  mkdir -p "$(dirname "$REPORT")"
  cargo llvm-cov --workspace --tests --lcov --output-path "$REPORT" -- --test-threads=1
fi

[ -f "$REPORT" ] || { echo "error: coverage report not found: $REPORT" >&2; exit 2; }

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

[ -n "$ROWS" ] || { echo "error: no coverage records parsed from $REPORT (not an lcov tracefile?)" >&2; exit 2; }

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
exit "$GATE_RC"
