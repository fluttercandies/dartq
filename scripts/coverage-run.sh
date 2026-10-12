#!/usr/bin/env bash
# coverage-run.sh — two-run measured-floors coverage runner for the dartq
# workspace (the final-phase floors procedure).
#
# == The two-run procedure ==
# Run 1 measures:  scripts/coverage-run.sh --measure [--output FILE]
#    one llvm-cov pass; per-crate line coverage lands as a TSV beside the
#    report (measured.tsv).
# Bridge:          scripts/coverage-run.sh --floors-from-measured [--tsv FILE]
#    writes measured−5 back into scripts/coverage-floors.tsv, comments
#    preserved — the ratchet step between the two runs.
# Run 2 enforces:  scripts/coverage-run.sh --enforce
#    a fresh llvm-cov pass scored against the floors table; exit 1 lists
#    every deficit.
#
# == Delegation (single execution posture) ==
# Every llvm-cov execution is delegated to scripts/coverage-gate.sh — the one
# place that owns the run posture: RUSTC_WRAPPER="" sccache bypass, the
# isolated target/coverage-build tree, --offline, the §19.5 repro-tuple seal,
# and the byte-stable summary.txt this script parses. This script owns only
# the floors workflow; there is no second llvm-cov invocation shape here.
#
# == Workspace pass, not per-crate passes ==
# The gate runs ONE `cargo llvm-cov --workspace --tests` pass and folds the
# trace per crate. Per-crate `-p` passes were rejected: a crate's floor must
# reflect the coverage the whole suite delivers (engine integration tests hit
# parser/semantic lines; per-crate runs would understate those rows and would
# need llvm-profdata trace merging), and one workspace pass costs one cold
# instrumented rebuild instead of N.
#
# == Resource discipline (SERIAL PHASE ONLY) ==
# Each mode executes the ENTIRE workspace test suite under instrumentation.
# Run it single-process with an RSS watchdog (see
# docs/audits/serial-phase-runbook.md), never alongside development-phase
# lanes (AGENTS.md memory-incident discipline: the 80GB/90GB incidents were
# exactly this shape). Test binaries run one at a time (cargo serializes
# them); the gate pins `-- --test-threads=1`, inside this runner's ≤2 budget
# for the heavyweight instrumented suite. Never run --measure and --enforce
# concurrently: each is a full cold rebuild.
#
# == Floor semantics ==
# floor = int(measured percent) − 5, clamped to ≥ 0 — five points of
# regression headroom under each crate's measured baseline. `default` is
# policy, not measurement: never rewritten. Rows for crates that produced no
# coverage (LF:0 stubs) are preserved verbatim. The harness/other fold rows
# are measurement infrastructure and never become floors; measured rows with
# no existing floor row are appended at the end of the data section (just
# before `default`).
#
# Usage:
#   scripts/coverage-run.sh --measure [--output FILE] [--dry-run]
#   scripts/coverage-run.sh --floors-from-measured [--tsv FILE] [--floors FILE] [--dry-run]
#   scripts/coverage-run.sh --enforce [--floors FILE] [--dry-run]
#   scripts/coverage-run.sh --help
#
#   --measure              run 1: one llvm-cov pass (delegated to
#                          coverage-gate.sh, fully advisory via --floor 0),
#                          then derive the per-crate measured.tsv beside the
#                          report.
#   --floors-from-measured bridge: rewrite the floors table from a
#                          measured.tsv (floor = int(pct) − 5, clamped ≥ 0;
#                          comments and unmeasured rows preserved; `default`
#                          untouched). --dry-run prints the resulting table
#                          instead of writing.
#   --enforce              run 2: fresh llvm-cov pass scored against the
#                          floors table (coverage-gate.sh --enforce); exit 1
#                          lists every below-floor crate.
#   --output FILE          --measure only: lcov destination (default
#                          target/coverage/coverage.lcov); summary.txt and
#                          measured.tsv land beside it. For an archived
#                          baseline use a path under docs/audits/coverage/
#                          (docs/plans/coverage-gate.md section 4).
#   --tsv FILE             --floors-from-measured only: measured TSV to read
#                          (default: measured.tsv beside the --output report,
#                          i.e. target/coverage/measured.tsv by default).
#   --floors FILE          floors table (default scripts/coverage-floors.tsv):
#                          rewritten by --floors-from-measured, consumed by
#                          --enforce. Paths resolve from the repo root.
#   --dry-run              --measure / --enforce: delegate the gate's
#                          plan-only dry run (tool presence probe and floors
#                          validation, no suite execution).
#                          --floors-from-measured: compute and print, write
#                          nothing.
#   --help                 this text.
#
# Exit codes: 0 = step completed (or dry run); 1 = enforcing run red (a
# crate is below its floor, propagated from coverage-gate.sh); 2 = usage
# error, missing tool, or malformed measured/floors table.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GATE="$SCRIPT_DIR/coverage-gate.sh"
FLOORS_DEFAULT="$SCRIPT_DIR/coverage-floors.tsv"
REPORT_DEFAULT="target/coverage/coverage.lcov"

usage() {
  sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

MODE=""
DRY_RUN=0
FLOORS="$FLOORS_DEFAULT"
REPORT="$REPORT_DEFAULT"
MEASURED_TSV=""

while [ $# -gt 0 ]; do
  # Termination invariant: every arm consumes its tokens (>=1 per iteration)
  # or exits.
  case "$1" in
    --measure|--floors-from-measured|--enforce)
      [ -z "$MODE" ] || { echo "coverage-run: --measure/--floors-from-measured/--enforce are mutually exclusive (one step at a time; each measure/enforce step is a full serial-phase run)" >&2; exit 2; }
      MODE="${1#--}"; shift ;;
    --floors)
      [ $# -ge 2 ] || { echo "coverage-run: --floors needs a value" >&2; exit 2; }
      FLOORS="$2"; shift 2 ;;
    --output)
      [ $# -ge 2 ] || { echo "coverage-run: --output needs a value" >&2; exit 2; }
      REPORT="$2"; shift 2 ;;
    --tsv)
      [ $# -ge 2 ] || { echo "coverage-run: --tsv needs a value" >&2; exit 2; }
      MEASURED_TSV="$2"; shift 2 ;;
    --dry-run)
      DRY_RUN=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "coverage-run: unknown argument: $1 (see --help)" >&2; usage >&2; exit 2 ;;
  esac
done
[ -n "$MODE" ] || { echo "coverage-run: one of --measure, --floors-from-measured, --enforce is required" >&2; usage >&2; exit 2; }

# Flag/mode compatibility: a silently ignored flag is a config bug.
case "$MODE" in
  measure)
    [ -z "$MEASURED_TSV" ] || { echo "coverage-run: --tsv does not apply to --measure (measured.tsv is an output there; aim it with --output)" >&2; exit 2; }
    [ "$FLOORS" = "$FLOORS_DEFAULT" ] || { echo "coverage-run: --floors does not apply to --measure (the measurement run is fully advisory)" >&2; exit 2; } ;;
  floors-from-measured)
    [ "$REPORT" = "$REPORT_DEFAULT" ] || { echo "coverage-run: --output does not apply to --floors-from-measured (point --tsv at the measured TSV)" >&2; exit 2; } ;;
  enforce)
    [ "$REPORT" = "$REPORT_DEFAULT" ] || { echo "coverage-run: --output does not apply to --enforce" >&2; exit 2; }
    [ -z "$MEASURED_TSV" ] || { echo "coverage-run: --tsv does not apply to --enforce (floors come from --floors)" >&2; exit 2; } ;;
esac

# Repo root: everything below resolves from the workspace.
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"
[ -f Cargo.toml ] || { echo "coverage-run: no workspace Cargo.toml at $ROOT" >&2; exit 2; }

REPORT_DIR="$(dirname "$REPORT")"
MEASURED="${MEASURED_TSV:-$REPORT_DIR/measured.tsv}"

# Preflight: fail closed, tutorial-style, before any leg dispatches. The
# gate re-probes on its own; this makes the runner's failure mode
# self-explanatory without reading a second script.
command -v cargo >/dev/null 2>&1 \
  || { echo "coverage-run: cargo not on PATH; the coverage runner cannot run" >&2; exit 2; }
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "coverage-run: cargo-llvm-cov is not installed; the coverage runner cannot run." >&2
  echo "  install once: cargo install cargo-llvm-cov --locked" >&2
  echo "  also required: rustup component add llvm-tools-preview" >&2
  exit 2
fi
# llvm-tools supplies the llvm-profdata/llvm-cov binaries cargo-llvm-cov
# shells out to. Recent rustup names the component `llvm-tools` (host
# suffixed); either name satisfies the probe.
tools_ok=0
if command -v rustup >/dev/null 2>&1; then
  installed="$(rustup component list --installed 2>/dev/null || true)"
  case "$installed" in *llvm-tools*) tools_ok=1 ;; esac
fi
if [ "$tools_ok" = 0 ]; then
  sysroot="$(rustc --print sysroot 2>/dev/null || true)"
  if [ -n "$sysroot" ] && [ -n "$(find "$sysroot/lib/rustlib" -maxdepth 3 -type f \( -name llvm-profdata -o -name llvm-cov \) 2>/dev/null)" ]; then
    tools_ok=1
  fi
fi
if [ "$tools_ok" = 0 ]; then
  echo "coverage-run: the llvm-tools component is missing; cargo-llvm-cov needs llvm-profdata/llvm-cov from it." >&2
  echo "  install: rustup component add llvm-tools-preview" >&2
  echo "  (either the -preview or host-suffixed component name resolves)" >&2
  exit 2
fi

if [ "$MODE" = "measure" ]; then
  echo "== coverage-run: run 1 (measure) — delegated to coverage-gate.sh (--floor 0: fully advisory)"
  rc=0
  if [ "$DRY_RUN" = 1 ]; then
    bash "$GATE" --floor 0 --output "$REPORT" --dry-run || rc=$?
    printf '  %-14s %s\n' "derived" "$MEASURED (folded from summary.txt beside the report, after the real run)"
    exit "$rc"
  fi
  bash "$GATE" --floor 0 --output "$REPORT" || rc=$?
  [ "$rc" -eq 0 ] || exit "$rc"

  # Fold the gate's byte-stable summary table into the measured TSV.
  # Table rows are: name lines-hit lines-total pct% floor — keep the
  # product-crate rows only (drop the header, TOTAL, and the harness/other
  # measurement-infrastructure rows).
  [ -f "$REPORT_DIR/summary.txt" ] || { echo "coverage-run: gate summary not found: $REPORT_DIR/summary.txt" >&2; exit 2; }
  LC_ALL=C awk '
    NF == 5 && $4 ~ /%$/ && $5 ~ /^[0-9]+(\.[0-9]+)?$/ &&
    $1 != "crate" && $1 != "TOTAL" && $1 != "harness" && $1 != "other" {
      pct = $4; sub(/%$/, "", pct)
      printf "%s\t%s\t%s\t%s\n", $1, $2, $3, pct
    }
  ' "$REPORT_DIR/summary.txt" > "$MEASURED"
  [ -s "$MEASURED" ] || { echo "coverage-run: no product-crate rows parsed from $REPORT_DIR/summary.txt" >&2; exit 2; }

  echo "== coverage-run: measured per-crate line coverage -> $MEASURED"
  cat "$MEASURED"
  echo "coverage-run: next: scripts/coverage-run.sh --floors-from-measured --tsv $MEASURED"
  exit 0
fi

if [ "$MODE" = "floors-from-measured" ]; then
  [ -f "$MEASURED" ] || { echo "coverage-run: measured TSV not found: $MEASURED (run --measure first)" >&2; exit 2; }
  [ -f "$FLOORS" ] || { echo "coverage-run: floors table not found: $FLOORS" >&2; exit 2; }
  echo "== coverage-run: floors bridge (floor = int(measured) − 5, clamped ≥ 0; comments preserved)"

  # One pass reads the measured TSV, one buffers the floors table, END
  # validates + emits. Buffered rewrite keeps comments, blank lines,
  # `default`, and unmeasured rows byte-identical; measured crates with no
  # row are appended just before `default` in measured-file order (the fold
  # emits LC_ALL=C sorted). BSD-awk portable: no ARGIND, no asort.
  rewrite_floors='
    FNR == NR {
      if ($0 == "") next
      n = split($0, f, "\t")
      if (n != 4 || f[1] == "" || f[2] !~ /^[0-9]+$/ || f[3] !~ /^[0-9]+$/ || f[4] !~ /^[0-9]+(\.[0-9]+)?$/) {
        printf "coverage-run: malformed measured row %d: %s\n", FNR, $0 > "/dev/stderr"
        err = 1; exit 2
      }
      if (f[1] == "harness" || f[1] == "other" || f[1] == "TOTAL" || f[1] == "crate") {
        printf "coverage-run: %s is measurement infrastructure, not a floorable crate row\n", f[1] > "/dev/stderr"
        err = 1; exit 2
      }
      floor_pct[f[1]] = int(f[4]) - 5
      if (floor_pct[f[1]] < 0) floor_pct[f[1]] = 0
      morder[++mn] = f[1]
      next
    }
    { tlines[++tn] = $0; next }
    END {
      if (err) exit 2
      seen_default = 0
      for (i = 1; i <= tn; i++) {
        line = tlines[i]
        if (line == "" || line ~ /^#/) continue
        if (split(line, f, "\t") != 2 || f[1] == "" || f[2] !~ /^[0-9]+(\.[0-9]+)?$/) {
          printf "coverage-run: malformed floors row %d: %s\n", i, line > "/dev/stderr"
          exit 2
        }
        if (f[1] == "default") {
          if (seen_default++) { print "coverage-run: duplicate default row in floors table" > "/dev/stderr"; exit 2 }
        } else if (seen_row[f[1]]++) {
          printf "coverage-run: duplicate floor row: %s\n", f[1] > "/dev/stderr"
          exit 2
        }
      }
      if (!seen_default) { print "coverage-run: floors table has no `default` row" > "/dev/stderr"; exit 2 }
      pc = 0
      for (i = 1; i <= mn; i++) {
        c = morder[i]
        if (!(c in seen_row)) pend[++pc] = c
      }
      # Dry mode prints only the decision-relevant rows (updated + newly
      # inserted); unchanged comments/rows stay in the file on disk.
      if (dry) printf "%-28s %10s %10s\n", "crate", "old", "new(floor)"
      for (i = 1; i <= tn; i++) {
        line = tlines[i]
        if (line == "" || line ~ /^#/) { if (!dry) print line; continue }
        split(line, f, "\t")
        if (f[1] == "default") {
          for (j = 1; j <= pc; j++) {
            if (dry) printf "%-28s %10s %10d\n", pend[j], "(new)", floor_pct[pend[j]]
            else print pend[j] "\t" floor_pct[pend[j]]
          }
          if (!dry) print line
          continue
        }
        if (f[1] in floor_pct) {
          if (dry) printf "%-28s %10s %10d\n", f[1], f[2], floor_pct[f[1]]
          else printf "%s\t%d\n", f[1], floor_pct[f[1]]
        } else {
          if (!dry) print line
        }
      }
    }
  '
  if [ "$DRY_RUN" = 1 ]; then
    LC_ALL=C awk -v dry=1 "$rewrite_floors" "$MEASURED" "$FLOORS"
    echo "== dry run: floors table not modified"
  else
    LC_ALL=C awk -v dry=0 "$rewrite_floors" "$MEASURED" "$FLOORS" > "$FLOORS.tmp"
    mv "$FLOORS.tmp" "$FLOORS"
    echo "coverage-run: floors table updated: $FLOORS"
    echo "coverage-run: next: scripts/coverage-run.sh --enforce"
  fi
  exit 0
fi

# MODE = enforce
echo "== coverage-run: run 2 (enforce) — delegated to coverage-gate.sh --enforce"
rc=0
gate_args=(--enforce --floors "$FLOORS")
[ "$DRY_RUN" = 1 ] && gate_args+=(--dry-run)
bash "$GATE" "${gate_args[@]}" || rc=$?
exit "$rc"
