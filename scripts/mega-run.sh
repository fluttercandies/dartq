#!/usr/bin/env bash
# mega-run.sh — the S4-scale budget runner over the generated mega corpus
# (PRD section 19.2 S4 row + section 19.3 RSS/100k-LOC and Incremental P95
# axes).
#
# Per scale (1k / 10k / 100k, via --scale, default 1k; --all runs the
# ladder): it (a) regenerates the scale tree if absent (corpus-gen, pinned
# --seed 0 — the deterministic contract, see tools/corpus-gen/README.md),
# (b) materializes the hand-written .dart_tool/package_config.json the
# generated tree lacks (the harness corpus preflight requires it; the
# config lists the app package plus every packages/* entry, rootUri
# relative, languageVersion 3.13), and (c) runs the two existing harness
# bench faces EXACTLY as recorded — rss-100k (peak tree RSS normalized by
# LOC/100k, 8 GiB watchdog built in) and incremental-p95 (cold baseline +
# per-edit-step wall times over a materialized copy; the copy of the 100k
# scale is ~400 MB, plan disk accordingly).
#
# NO oracle differential by default: S4 acceptance is a dartq-side budget
# (RSS ceiling + latency), not a Jaccard row — `dart analyze` over 100k
# files is not the acceptance shape (PRD section 19.2 S4; the differential
# axis stays with the m0/m1 gates).
#
# SERIAL PHASE ONLY: a run spawns real dartq processes over up to ~10k
# real files (100k-scale trees are ~400 MB on disk). Never run it alongside
# development-phase lanes (memory-incident discipline). --dry-run resolves
# every path, prints the plan and the exact commands, and executes nothing
# — no regeneration, no config write, no bench.
#
# Usage:
#   scripts/mega-run.sh [--scale 1k|10k|100k] [--all] [--dry-run]
#                       [--label L] [--help]
#
#   --scale S       single scale to run (default 1k).
#   --all           run the whole ladder (1k, 10k, 100k) in order.
#   --dry-run       print the plan + exact commands, execute nothing.
#   --label L       report directory name under docs/audits/mega/.
#                   Default: MEGA_LABEL env, else mega-r1. Reports land at
#                   docs/audits/mega/<label>/<scale>/{rss-100k,
#                   incremental-p95}/report.json plus edit-manifest.json.
#
# Environment overrides: DARTQ_CMD (the dartq side; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`, the m1-gate shape), FVM_HOME (default ~/fvm),
# DARTQ_HARNESS_ORACLE_TIMEOUT_MS (forwarded to the harness drivers;
# default raised to 600000 here — a 100k-file batch analysis exceeds the
# harness's 120 s default).
#
# Exit codes: 0 every requested scale completed with both reports; 1
# RESERVED (budget thresholds are not asserted yet — no baseline exists;
# see the pending-thresholds note in the summary); 2 usage/environment
# error (missing binaries, jq missing, regeneration failure, missing or
# unreadable reports) including the harness's own error tier.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

OUT_ROOT="docs/audits/mega"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="mega-r1"
DEFAULT_TIMEOUT_MS=600000

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "mega-run: $*" >&2
  exit 2
}

dry_run=0
all_scales=0
label="${MEGA_LABEL:-$DEFAULT_LABEL}"
scale=""
while [ $# -gt 0 ]; do
  case "$1" in
  --scale)
    [ $# -ge 2 ] || die "--scale needs a value"
    scale="$2"
    shift
    ;;
  --all) all_scales=1 ;;
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || die "--label needs a value"
    label="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/mega-run.sh --help)"
    ;;
  esac
  shift
done

if [ "$all_scales" -eq 1 ] && [ -n "$scale" ]; then
  die "--all and --scale are mutually exclusive"
fi
if [ "$all_scales" -eq 0 ]; then
  [ -n "$scale" ] || scale="1k"
fi
if [ "$all_scales" -eq 1 ]; then
  scales="1k 10k 100k"
else
  scales="$scale"
fi
for s in $scales; do
  case "$s" in
  1k | 10k | 100k) ;;
  *) die "invalid --scale '$s' (one of: 1k, 10k, 100k)" ;;
  esac
done

# --- preflight ---------------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  die "jq is required for config materialization and the summary (brew install jq)"
fi
if [ ! -x "$HARNESS" ]; then
  die "$HARNESS not found — build it first: cargo build -p harness"
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  die "./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)"
fi

need_regen=""
for s in $scales; do
  if [ ! -f "tests/corpus/mega/$s/manifest.json" ]; then
    need_regen="$need_regen $s"
  fi
done
if [ -n "$need_regen" ] && [ "$dry_run" -eq 0 ] && ! command -v cargo >/dev/null 2>&1; then
  die "cargo is required to regenerate missing scales:$need_regen"
fi

# A 100k-file batch analysis exceeds the harness's 120 s default budget.
if [ -z "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS:-}" ]; then
  export DARTQ_HARNESS_ORACLE_TIMEOUT_MS="$DEFAULT_TIMEOUT_MS"
fi

# --- helpers (run path only; --dry-run never calls these) --------------------

# Regenerate one scale with the pinned seed if its tree is absent.
regen_scale() {
  s="$1"
  dir="tests/corpus/mega/$s"
  if [ -f "$dir/manifest.json" ]; then
    return 0
  fi
  echo "-- [$s] tree absent; regenerating (corpus-gen --seed 0)"
  cargo run -q -p corpus-gen -- --out "$dir" --scale "$s" --seed 0 || return 1
}

# Materialize the hand-written package_config.json the generated tree lacks:
# the app package plus every packages/* entry, config-file-relative rootUri,
# languageVersion 3.13 (the harness corpus preflight's floor). Idempotent.
materialize_config() {
  s="$1"
  dir="tests/corpus/mega/$s"
  config="$dir/.dart_tool/package_config.json"
  if [ -f "$config" ]; then
    return 0
  fi
  echo "-- [$s] materializing $config"
  mkdir -p "$dir/.dart_tool" || return 1
  entries='[{"name":"app","rootUri":"..","packageUri":"lib/","languageVersion":"3.13"}]'
  for d in "$dir"/packages/*/; do
    [ -f "${d}pubspec.yaml" ] || continue
    name="$(sed -n 's/^name: *//p' "${d}pubspec.yaml" | head -n1)"
    [ -n "$name" ] || continue
    rel="../${d#"$dir"/}"
    rel="${rel%/}"
    entries="$(printf '%s' "$entries" | jq -c --arg n "$name" --arg r "$rel" \
      '. + [{name: $n, rootUri: $r, packageUri: "lib/", languageVersion: "3.13"}]')" || return 1
  done
  jq -n --argjson p "$entries" '{configVersion: 2, packages: $p}' >"$config" || return 1
}

# The budget-smoke edit manifest for incremental-p95: five whole-file edits
# to the entry point (stable across scales), each step growing the file.
# Deliberately minimal — the acceptance run can replace this file.
write_manifest() {
  outdir="$1"
  mkdir -p "$outdir" || return 1
  cat >"$outdir/edit-manifest.json" <<'EOF'
{
  "steps": [
    {"edits": [{"path": "lib/main.dart", "content": "void main() => print('mega bench step 1');\n"}]},
    {"edits": [{"path": "lib/main.dart", "content": "class ScreenA {\n  final String id;\n  const ScreenA(this.id);\n}\n\nvoid main() => print(ScreenA('a').id);\n"}]},
    {"edits": [{"path": "lib/main.dart", "content": "class ScreenA {\n  final String id;\n  const ScreenA(this.id);\n}\n\nclass ScreenB {\n  final ScreenA inner;\n  const ScreenB(this.inner);\n}\n\nvoid main() => print(ScreenB(ScreenA('b')).inner.id);\n"}]},
    {"edits": [{"path": "lib/main.dart", "content": "sealed class Node {}\n\nclass Leaf extends Node {\n  final int value;\n  Leaf(this.value);\n}\n\nString describe(Node node) => switch (node) {\n      Leaf(:final value) => 'leaf $value',\n    };\n\nvoid main() => describe(Leaf(1));\n"}]},
    {"edits": [{"path": "lib/main.dart", "content": "Future<void> run() async {\n  await for (final _ in Stream<int>.fromIterable(const [1, 2, 3])) {}\n}\n\nvoid main() => run();\n"}]}
  ]
}
EOF
}

# --- plan --------------------------------------------------------------------
echo "== mega-run plan"
printf '  %-16s %s\n' "scales" "$scales"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s\n' "dartq" "$DARTQ_CMD"
printf '  %-16s %s (exported default; unset DARTQ_HARNESS_ORACLE_TIMEOUT_MS to keep the harness 120s)\n' \
  "timeout_ms" "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS:-<harness default>}"
for s in $scales; do
  dir="tests/corpus/mega/$s"
  if [ -f "$dir/manifest.json" ]; then
    state="present"
  else
    state="ABSENT -> will regenerate: cargo run -p corpus-gen -- --out $dir --scale $s --seed 0"
  fi
  printf '  %-16s %s (%s)\n' "scale $s" "$dir" "$state"
done
printf '  %-16s %s\n' "config" ".dart_tool/package_config.json materialized per scale if absent (harness preflight)"
printf '  %-16s two bench faces per scale: rss-100k + incremental-p95 (edit-manifest.json written into the report dir)\n' "metrics"
printf '  %-16s NO oracle differential (S4 is a dartq-side budget: PRD 19.2 S4, 19.3)\n' "oracle"
printf '  %-16s docs/audits/mega/%s/<scale>/\n' "reports" "$label"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: plan resolved, nothing executed"
  exit 0
fi

# --- run ---------------------------------------------------------------------
for s in $scales; do
  dir="tests/corpus/mega/$s"
  outdir="$OUT_ROOT/$label/$s"
  echo
  echo "== [$label/$s] mega budget run"
  regen_scale "$s" || die "regeneration of $s failed"
  materialize_config "$s" || die "config materialization for $s failed"
  write_manifest "$outdir" || die "writing the edit manifest for $s failed"

  started=$SECONDS
  "$HARNESS" bench --metric rss-100k --corpus-dir "$dir" --out "$outdir" --dartq-cmd "$DARTQ_CMD" ||
    die "bench rss-100k failed for $s (harness error; see its stderr)"
  "$HARNESS" bench --metric incremental-p95 --corpus-dir "$dir" --out "$outdir" \
    --manifest "$outdir/edit-manifest.json" --dartq-cmd "$DARTQ_CMD" ||
    die "bench incremental-p95 failed for $s (harness error; see its stderr)"
  wall=$((SECONDS - started))

  # --- summary ---------------------------------------------------------------
  rss="$(jq -r '[.file_count, .loc, .peak_rss_kib, .rss_per_100k_loc_kib, .samples] | @tsv' \
    "$outdir/rss-100k/report.json" 2>/dev/null)" || rss=""
  inc="$(jq -r '[.baseline_duration_ms, .p50_ms, .p95_ms, .p99_ms, (.steps | length)] | @tsv' \
    "$outdir/incremental-p95/report.json" 2>/dev/null)" || inc=""
  if [ -z "$rss" ] || [ -z "$inc" ]; then
    die "report(s) under $outdir are missing or unreadable"
  fi
  IFS="$(printf '\t')" read -r files loc peak_rss per_100k samples <<EOF
$rss
EOF
  IFS="$(printf '\t')" read -r baseline p50 p95 p99 steps <<EOF
$inc
EOF
  echo
  echo "== [$label/$s] summary"
  printf '  %-16s %ss\n' "wall_time" "$wall"
  printf '  %-16s %s files, %s LOC\n' "corpus" "$files" "$loc"
  printf '  %-16s %s KiB peak (%s samples) = %s KiB per 100k LOC\n' "rss_100k" "$peak_rss" "$samples" "$per_100k"
  printf '  %-16s baseline %sms | %s steps: p50 %sms p95 %sms p99 %sms\n' \
    "incremental" "$baseline" "$steps" "$p50" "$p95" "$p99"
  printf '  %-16s pending — thresholds assert in the acceptance run once a baseline exists\n' "budget_check"
  echo "  reports: $outdir/{rss-100k,incremental-p95}/report.json"
done

echo
echo "mega-run: VERDICT COMPLETE (no thresholds asserted; exit 1 reserved for the acceptance run)"
exit 0
