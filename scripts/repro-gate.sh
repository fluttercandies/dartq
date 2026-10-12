#!/usr/bin/env bash
# repro-gate.sh — the PRD §19.5 CI reproducibility gate (goal #799).
#
# Composes the repro trio into one gate: builds the dartq CLI and the
# harness, runs `harness repro` (fresh-process N-run byte-identical
# normalized-digest check) over a corpus, then seals the judged run with
# scripts/repro-tuple.sh. The {tuple, output_digest} pair lands under
# docs/audits/repro/<label>/seal.json — timestamp-free, so two runs of the
# same repository and toolchain state produce byte-identical seals a CI
# assertion can compare directly.
#
# Clean-tree guard: the tuple's identity scope is tracked-file content
# (git head + `git diff HEAD` patch hash — see repro-tuple.sh), so the gate
# refuses to run from a dirty tracked tree: a seal taken over uncommitted
# changes names an identity no clean checkout can reproduce. Untracked
# workspace material (docs/, .third/, .goal/) is not part of the identity
# and does not block the gate. The guard enforces on the run path only.
#
# SERIAL PHASE ONLY: a run builds workspace crates and spawns real `dartq
# analyze` processes over the corpus. Never run it alongside
# development-phase lanes (memory-incident discipline). --dry-run resolves
# every path and executes nothing (no guard enforcement, no build, no run).
#
# Usage:
#   scripts/repro-gate.sh [--dry-run] [--label L]
#   scripts/repro-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact
#                   commands, execute nothing.
#   --label L       report directory name under docs/audits/repro/.
#                   Default: default.
#
# Environment overrides: REPRO_CORPUS (corpus dir; default tests/corpus/m0,
# point it at the full mega/1k face for the heavy run), FVM_HOME (default
# ~/fvm), DARTQ_CMD (the dartq side of the harness; default the m0-gate
# shape `./target/release/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>` — the release face the m0 corpus was e2e-verified with).
#
# Exit codes: 0 gate met (all runs byte-identical, seal written); 1
# divergence (harness repro exit 1 — the report names the first divergent
# row); 2 usage or environment error, dirty tracked tree, build failure, or
# the harness's own error tier (dartq crash, corpus preflight failure).

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="${REPRO_CORPUS:-tests/corpus/m0}"
RUNS=2
OUT_ROOT="docs/audits/repro"
HARNESS="./target/release/harness"
DEFAULT_LABEL="default"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
label="$DEFAULT_LABEL"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "repro-gate: --label needs a value" >&2
      exit 2
    }
    label="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "repro-gate: unknown argument '$1' (see scripts/repro-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
if [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "repro-gate: $CORPUS is not a corpus package — run: cargo run -p harness -- corpus" >&2
  exit 2
fi
if [ ! -d "$CORPUS/.dart_tool" ]; then
  echo "repro-gate: $CORPUS has no .dart_tool — assemble + pub get the corpus first (see README)" >&2
  exit 2
fi
command -v git >/dev/null 2>&1 || { echo "repro-gate: git is required for the clean-tree guard and the tuple" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "repro-gate: jq is required for the seal (brew install jq)" >&2; exit 2; }

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
DEFAULT_DARTQ_CMD="./target/release/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"

# --- plan --------------------------------------------------------------------
outdir="$OUT_ROOT/$label"
report="$outdir/repro-report.json"
seal="$outdir/seal.json"
echo "== repro-gate plan"
printf '  %-16s %s\n' "corpus" "$CORPUS"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s fresh-process runs, byte-identical normalized digest\n' "runs" "$RUNS"
printf '  %-16s %s\n' "guard" "clean tracked tree (git status --porcelain --untracked-files=no)"
printf '  %-16s %s\n' "build" "cargo build --release -p dartq-cli -p harness"
printf '  %-16s harness repro --corpus-dir %s --dartq-cmd "%s" --runs %s --out %s\n' \
  "command" "$CORPUS" "$DARTQ_CMD" "$RUNS" "$outdir"
printf '  %-16s scripts/repro-tuple.sh --output %s -> %s\n' "seal" "$report" "$seal"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing executed"
  exit 0
fi

# --- clean-tree guard ----------------------------------------------------------
# Same tracked-files-only predicate repro-tuple.sh uses for its dirty flag.
dirty="$(git status --porcelain --untracked-files=no)"
if [ -n "$dirty" ]; then
  echo "repro-gate: tracked worktree is dirty — the tuple's identity scope is tracked files" >&2
  echo "  (git head + git diff HEAD patch hash); a seal over uncommitted changes is" >&2
  echo "  an identity no clean checkout can reproduce. Commit or stash first." >&2
  git --no-pager diff HEAD --stat | sed 's/^/  /' >&2
  exit 2
fi

# --- build ---------------------------------------------------------------------
echo
echo "== build: cargo build --release -p dartq-cli -p harness"
if ! cargo build -q --release -p dartq-cli -p harness; then
  echo "repro-gate: build FAILED (dartq-cli / harness)" >&2
  exit 2
fi
if [ ! -x "$HARNESS" ]; then
  echo "repro-gate: $HARNESS not found after the build" >&2
  exit 2
fi

# --- run the gate --------------------------------------------------------------
# harness repro exit codes: 0 = all runs byte-identical, 1 = divergence
# (report written), 2 = harness-own error (dartq crash, corpus preflight).
echo
echo "== [$label] repro gate"
mkdir -p "$outdir"
# Capture the harness exit status at invocation: a scored FAIL (exit 1) must
# never be misread as 0 — the idiom m0-gate fixed after the else-less-if bug.
rc=0
"$HARNESS" repro --corpus-dir "$CORPUS" --dartq-cmd "$DARTQ_CMD" --runs "$RUNS" --out "$outdir" || rc=$?
if [ "$rc" -eq 0 ]; then
  echo "-- [$label] $RUNS/$RUNS runs byte-identical"
elif [ "$rc" -eq 1 ]; then
  echo "repro-gate: VERDICT FAIL — runs diverged (harness exit 1); first divergent row in: $report" >&2
  exit 1
else
  echo "repro-gate: ERRORED (harness exit $rc, harness-own failure)" >&2
  exit 2
fi

# --- seal the judged run ---------------------------------------------------------
digest="$(jq -r '.output_digest // empty' "$report" 2>/dev/null)"
if [ -z "$digest" ]; then
  echo "repro-gate: report $report is missing or unreadable (no output_digest)" >&2
  exit 2
fi
tuple="$(scripts/repro-tuple.sh --output "$report")" || {
  echo "repro-gate: repro-tuple.sh failed sealing $report" >&2
  exit 2
}
if ! jq -n --argjson tuple "$tuple" --arg digest "$digest" '$tuple + {output_digest: $digest}' > "$seal"; then
  echo "repro-gate: writing $seal failed" >&2
  exit 2
fi

echo
echo "== repro-gate summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s\n' "output_digest" "$digest"
printf '  %-16s %s\n' "report" "$report"
printf '  %-16s %s\n' "seal" "$seal"
printf '  %-16s %s\n' "tuple" "$tuple"
echo "repro-gate: VERDICT PASS ($RUNS/$RUNS fresh-process runs byte-identical; seal: $seal)"
exit 0
