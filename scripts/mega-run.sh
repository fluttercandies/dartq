#!/usr/bin/env bash
# mega-run.sh — the S4-scale budget runner over the generated mega corpus
# (S4 acceptance axes: RSS/100k-LOC and Incremental P95 budgets).
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
# files is not the S4 acceptance shape (the differential axis stays with
# the m0/m1 gates).
#
# --oracle-leg runs the mega T2 oracle differential leg at the 1k tier only
#. It materializes
# the seeded-error variant of the 1k tree (scripts/mega-seed-errors.sh from
# the checked-in manifest scripts/mega-seeded-errors.json) and drives the
# existing harness differential face twice, once per tree:
#   - clean tree tests/corpus/mega/1k -> <out>/oracle-clean/ — the record
#     face: oracle version + vacuity regime are read off the report (the
#     ruled clean regime passes --allow-vacuous, so the leg cannot die on
#     an empty oracle side) plus the false-positive detector;
#   - seeded tree tests/corpus/mega/1k-seeded -> <out>/oracle-seeded/ — the
#     judgment face: the manifest's injections MUST reach the oracle side
#     (no --allow-vacuous; a vacuous oracle side here means the injections
#     never landed and fails the leg).
# A9 asserts ERROR (code,file) Jaccard >= 1.0 on both faces via the
# harness's own threshold gate (harness exit 0). The leg also measures the
# A7 cold pair (#798): tests/bench/oracle_baseline.sh driven twice over the
# clean 1k tree — the oracle side (oracle-of-record dart,
# `dart analyze --format=machine`, passed via --cmd so both legs share one
# JSON schema) and the dartq side (DARTQ_CMD, the m1-gate shape) — 5 rounds
# each, round 1 = cold, captured as
# <out>/cold/{oracle,dartq}.jsonl (round+summary records; identical schema,
# diag_lines = the command's stdout line count on both sides). A7 is
# record-only here: the assert (dartq <= 1/2 oracle) is the acceptance
# ruling's call. A8 (10k/100k oracle DNF record, arbitration = A1) remains a
# SKIP row: it is the stress-tier record, not this leg. Requires --scale 1k
# (the leg is 1k-only; --all is refused). Without the flag nothing
# oracle-related runs and the strict table keeps the A7/A8/A9 SKIP row
# verbatim.
#
# --strict turns the run into the acceptance gate face (the strict
# acceptance ledger): per-scale budget rows are asserted and the run gets a
# labeled VERDICT. Asserted faces: A1 rss_per_100k_loc_kib <= 200000 KiB at
# the 100k tier (the RSS charter row; 1k/10k stay report-only per
# A2 — fixed process overhead dominates there), A3 the 8 GiB watchdog cap,
# A4/A5/A6 the batch-proxy incremental p95 bars (2000/4000/60000 ms by
# tier, adopted at recon closure #733), A10 zero-panic, A11 completion
# inside the driver timeout, A12 corpus fingerprint echo + seed-0 identity.
# With --oracle-leg, A9 (ERROR Jaccard >= bar on the 1k clean + seeded
# faces) asserts too. Without the leg the oracle faces (A7/A8/A9) and the
# S4 LOC accounting (A13) degrade to explicit SKIP rows — A7/A8/A9 belong
# to the mega T2 oracle leg, A13 to the parked T4 ruling. Bars are
# env-overridable (MEGA_* below) so a baseline run can relax one
# deliberately. Without --strict the runner asserts nothing
# (warm-up/baseline shape; never exits 1).
#
# SERIAL PHASE ONLY: a run spawns real dartq processes over up to ~10k
# real files (100k-scale trees are ~400 MB on disk). Never run it alongside
# development-phase lanes (memory-incident discipline). --dry-run resolves
# every path, prints the plan and the exact commands, and executes nothing
# — no regeneration, no config write, no bench.
#
# Usage:
#   scripts/mega-run.sh [--scale 1k|10k|100k] [--all] [--strict]
#                       [--oracle-leg] [--dry-run] [--label L] [--help]
#
#   --scale S       single scale to run (default 1k).
#   --all           run the whole ladder (1k, 10k, 100k) in order.
#   --strict        acceptance gate face:
#                   assert the budget rows, print a VERDICT line, exit 1 on
#                   any breach. Under a strict 100k tier a disk-headroom
#                   precheck (~800 MB for the incremental copy) runs first.
#   --oracle-leg    1k-only oracle differential leg (mega T2, #736): seed
#                   the 1k tree from the checked-in manifest, then run the
#                   harness differential face on the clean tree (record
#                   face, --allow-vacuous) and the seeded tree (judgment
#                   face, vacuity refused). A9 asserts ERROR (code,file)
#                   Jaccard >= bar on both. Also measures the A7 cold pair
#                   (#798): oracle_baseline.sh twice over the clean 1k tree
#                   (oracle-of-record dart vs DARTQ_CMD, 5 rounds each,
#                   round 1 = cold; single-instance, sequential; a 30 s
#                   oracle canary precedes the sides, and each side runs
#                   under a 900 s wall cap — a hung canary or round is
#                   group-killed and fails the leg). Reports
#                   land at target/evidence/mega/<label>/1k/{oracle-clean,
#                   oracle-seeded,cold}/ — the cold dir also carries the
#                   canary record and the pinned round-outputs work dir.
#                   Mutually exclusive with --all and scales other than 1k.
#   --dry-run       print the plan + exact commands, execute nothing
#                   (--dry-run --strict prints the asserted table).
#   --label L       report directory name under target/evidence/mega/.
#                   Default: MEGA_LABEL env, else mega-r1. A label whose
#                   directory already exists refuses the run (exit 2) —
#                   recorded evidence is never overwritten. Reports land at
#                   target/evidence/mega/<label>/<scale>/rss-100k/<corpus>/
#                   report.json (corpus = the scale name),
#                   <scale>/incremental-p95/report.json, plus
#                   edit-manifest.json.
#
# Environment overrides: DARTQ_CMD (the dartq side; default
# `./target/debug/dartq analyze --format=machine --sdk <fvm default
# dart-sdk>`, the m1-gate shape), FVM_HOME (default ~/fvm; the leg needs
# its dart binary as the oracle), DARTQ_HARNESS_ORACLE_TIMEOUT_MS
# (forwarded to the harness drivers; default raised to 600000 here — a
# 100k-file batch analysis exceeds the harness's 120 s default).
# --strict bar overrides (non-negative integers, validated up front):
# MEGA_RSS_100K_LIMIT_KIB (A1, KiB), MEGA_INC_P95_1K_MS
# / MEGA_INC_P95_10K_MS / MEGA_INC_P95_100K_MS (A4/A5/A6, ms). Oracle-leg
# bar override (decimal in [0.0, 1.0]): MEGA_ORACLE_JACCARD_MIN (A9,
# default 1.0).
#
# Exit codes: 0 every requested scale completed with both reports — and,
# under --strict, every asserted acceptance face passed; 1 a --strict
# acceptance breach (watchdog kill, driver timeout, dartq crash, a budget
# row over its bar, or — under --oracle-leg — an A9 Jaccard below its bar
# or a vacuous seeded face: the code #546 reserved for the acceptance run;
# a non-strict run never exits 1); 2 usage/environment error (missing
# binaries, jq missing, regeneration or seeding failure, missing or
# unreadable reports, invalid MEGA_* bar, failed disk-headroom precheck,
# corpus identity failure, oracle dart missing, an existing report label)
# including the harness's own error tier.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

OUT_ROOT="target/evidence/mega"
HARNESS="./target/debug/harness"
DEFAULT_LABEL="mega-r1"
DEFAULT_TIMEOUT_MS=600000

# --- strict acceptance bars (asserted only under --strict; defined below) -------------------------------------------------
# A1: (the RSS charter row) "RSS/100k LOC <= 200 MB" — decimal reading,
# 200000 KiB (stricter than the MiB reading, 204800). Asserted at the 100k
# tier only. Env override: MEGA_RSS_100K_LIMIT_KIB.
RSS_100K_LIMIT_KIB=200000
# A3: tree-RSS watchdog cap the harness itself enforces (runbook:8;
# tests/harness/src/bench.rs WATCHDOG_THRESHOLD_KIB). A breach kills the
# driver group and fails the bench; strict mode classifies it as a FAIL.
WATCHDOG_CAP_KIB=8388608
# A4/A5/A6: batch-proxy incremental p95 bars by tier. The metric spawns one
# full dartq process per edit step (tests/harness/src/bench.rs), so
# the charter's resident-2s row binds only as the adopted analog. A4 1k:
# the resident-2s charter row analog at the 52k-LOC tier; A5 10k: 2x linear from the pinned
# 250k point; A6 100k: adopted two-stage (r1 records the baseline, the bar
# asserts from r2 on; relax via env for a deliberate r1 run). Env overrides:
# MEGA_INC_P95_1K_MS / MEGA_INC_P95_10K_MS / MEGA_INC_P95_100K_MS.
INC_P95_1K_MS=2000
INC_P95_10K_MS=4000
INC_P95_100K_MS=60000
# Strict 100k disk precheck: the incremental-p95 face copies the scale dir
# before timing it (~398 MB corpus + the copy; tools/corpus-gen/README.md
# consumption note and the #546 ledger's deferred note).
DISK_HEADROOM_100K_MIB=800
# A9 (oracle leg, 1k only): ERROR (code,file) Jaccard bar.
# Bar 1.0 — full parity like every differential gate
# (2026-10-12 ruling, the full-parity bars ADR;
# supersedes the earlier numeric bars; residuals pass only via individually
# adjudicated pinned exemptions, never a numeric budget). The harness
# enforces it as its own --threshold; this script classifies the
# harness exit. Env override: MEGA_ORACLE_JACCARD_MIN (decimal in [0,1]).
ORACLE_JACCARD_MIN=1.0
# Oracle leg inputs: the injector + the checked-in injection manifest (the
# seeded tree's single source of truth) and the derived tree it produces.
SEED_SCRIPT="scripts/mega-seed-errors.sh"
SEED_MANIFEST="scripts/mega-seeded-errors.json"
SEED_DIR="tests/corpus/mega/1k-seeded"
# A7 cold leg (#798): rounds per side via tests/bench/oracle_baseline.sh
# (round 1 = cold, rounds 2..N = hot; the plan's A7 binds 3+ reps — the
# instrument's default 5 covers it with margin). Both sides get the same
# count so the round records line up.
COLD_ROUNDS=5
# A7 cold-leg wall cap per side (seconds). oracle_baseline.sh has no timeout
# of its own and the serial chain's watchdog enforces RSS only, so the cap
# converts a hung analysis into a red leg. Per side, not per round: the
# instrument runs all rounds in one session (rounds 2..N are the
# same-session hot records the A7 face exists to record), so per-round caps
# would corrupt the hot-round evidence — the 30 s canary below bounds the
# startup-hang case instead. 900 s recalibrates the old flat 600 s (#1246):
# a 100k cold round measured 633 s of legitimate work against the old cap,
# so this clears the known-good longest round with headroom while staying
# ~300x the 1k tier's ~3 s cold round. Bigger tiers need the cap revisited
# alongside the 1k-only flag gate.
COLD_WALL_CAP_S=900
# A7 canary preflight bound (seconds): one exact-shape oracle round against
# a bare one-file corpus before the measured sides — a hung oracle fails
# here in 30 s with captured evidence instead of burning the side cap on
# round 1 (#1246).
COLD_CANARY_CAP_S=30
# Oracle-of-record caliber (mirrors m1-gate.sh; a mismatch is a WARN, not
# fatal — the report meta records the real banner either way).
EXPECTED_ORACLE_VERSION="3.13.1"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "mega-run: $*" >&2
  exit 2
}

dry_run=0
strict=0
all_scales=0
oracle_leg=0
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
  --strict) strict=1 ;;
  --oracle-leg) oracle_leg=1 ;;
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

# --- oracle leg: resolve + validate up front (both modes — the flag feeds
# the harness's own threshold gate either way) --------------------------------
if [ "$oracle_leg" -eq 1 ]; then
  if [ "$all_scales" -eq 1 ] || [ "$scales" != "1k" ]; then
    die "--oracle-leg runs the 1k tier only; use --scale 1k"
  fi
  oracle_jac_min="${MEGA_ORACLE_JACCARD_MIN:-$ORACLE_JACCARD_MIN}"
  case "$oracle_jac_min" in
  '' | *[!0-9.]*) die "MEGA_ORACLE_JACCARD_MIN must be a decimal in [0.0, 1.0] (got '$oracle_jac_min')" ;;
  esac
  if ! awk -v v="$oracle_jac_min" 'BEGIN { exit !(v >= 0.0 && v <= 1.0) }'; then
    die "MEGA_ORACLE_JACCARD_MIN must be in [0.0, 1.0] (got '$oracle_jac_min')"
  fi
  [ -f "$SEED_SCRIPT" ] || die "$SEED_SCRIPT not found (the T2 injection driver)"
  [ -f "$SEED_MANIFEST" ] || die "$SEED_MANIFEST not found (the checked-in seeded-error manifest)"
fi

# --- strict bars: resolve + validate up front, so a bad MEGA_* value fails
# before any serial-phase work (usage/environment tier, exit 2) ---------------
if [ "$strict" -eq 1 ]; then
  rss_limit_kib="${MEGA_RSS_100K_LIMIT_KIB:-$RSS_100K_LIMIT_KIB}"
  inc_p95_1k_ms="${MEGA_INC_P95_1K_MS:-$INC_P95_1K_MS}"
  inc_p95_10k_ms="${MEGA_INC_P95_10K_MS:-$INC_P95_10K_MS}"
  inc_p95_100k_ms="${MEGA_INC_P95_100K_MS:-$INC_P95_100K_MS}"
  for pair in \
    "MEGA_RSS_100K_LIMIT_KIB=$rss_limit_kib" \
    "MEGA_INC_P95_1K_MS=$inc_p95_1k_ms" \
    "MEGA_INC_P95_10K_MS=$inc_p95_10k_ms" \
    "MEGA_INC_P95_100K_MS=$inc_p95_100k_ms"; do
    name="${pair%%=*}"
    val="${pair#*=}"
    case "$val" in
    '' | *[!0-9]*) die "$name must be a non-negative integer (got '$val')" ;;
    esac
  done
fi

# --- preflight ---------------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  die "jq is required for config materialization and the summary (brew install jq)"
fi
if [ ! -x "$HARNESS" ]; then
  die "$HARNESS not found — build it first: cargo build -p harness"
fi

FVM_HOME="${FVM_HOME:-$HOME/fvm}"
sdk_root="$FVM_HOME/default/bin/cache/dart-sdk"
dart_bin="$sdk_root/bin/dart"
# Pin the oracle binary to its physical path: the fvm `default` component is
# a symlink, and the symlinked argv[0] was the one process-level input
# unique to the A7 round-1 hang (#1246). Resolving kills that divergence
# and pins which binary ran into every plan line and record. Guarded so a
# missing install keeps the fvm-shaped path for the die below.
if dart_dir="$(cd "$(dirname "$dart_bin")" 2>/dev/null && pwd -P)"; then
  dart_bin="$dart_dir/dart"
fi
DEFAULT_DARTQ_CMD="./target/debug/dartq analyze --format=machine --sdk $sdk_root"
DARTQ_CMD="${DARTQ_CMD:-$DEFAULT_DARTQ_CMD}"
if [ "$DARTQ_CMD" = "$DEFAULT_DARTQ_CMD" ] && [ ! -x "./target/debug/dartq" ]; then
  die "./target/debug/dartq not found — build it first: cargo build -p dartq-cli (or set DARTQ_CMD=<cmd>)"
fi
if [ "$oracle_leg" -eq 1 ] && [ ! -x "$dart_bin" ]; then
  die "oracle dart not found at $dart_bin (FVM_HOME=$FVM_HOME) — install the fvm default pin or point FVM_HOME at it"
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

# Strict run path: the 100k incremental-p95 face copies the scale dir before
# timing it — refuse to start without the headroom (environment tier, exit 2;
# --dry-run only prints the current availability).
if [ "$strict" -eq 1 ]; then
  case " $scales " in
  *" 100k "*)
    set -- $(df -k . | tail -n 1)
    if [ "$#" -lt 4 ]; then
      die "cannot read free disk space (df -k .)"
    fi
    need_kib=$((DISK_HEADROOM_100K_MIB * 1024))
    if [ "$dry_run" -eq 1 ]; then
      echo "-- disk headroom now: $4 KiB free (the run path needs >= $need_kib KiB for the 100k incremental copy)"
    elif [ "$4" -lt "$need_kib" ]; then
      die "insufficient disk headroom for the 100k incremental copy: $4 KiB free, need >= $need_kib KiB ($DISK_HEADROOM_100K_MIB MiB; tools/corpus-gen/README.md consumption note)"
    else
      echo "-- disk headroom ok: $4 KiB free (need >= $need_kib KiB for the 100k incremental copy)"
    fi
    ;;
  esac
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

# Classify one failed bench invocation for the strict gate. Driver-level
# failures are acceptance breaches — the
# harness's own markers identify which face broke: watchdog kill (A3),
# wall-clock timeout (A11), dartq exit outside the valid analysis tiers
# {0,1,2,3} (A10 — crash or panic). A harness-own failure (launch error, ps
# failure, report write) is an environment error instead. Sets $failure_note
# and returns 0 for a breach; returns 1 for harness-own.
classify_bench_failure() {
  if grep -q "RSS watchdog" "$errfile"; then
    failure_note="watchdog killed the driver tree at the ${WATCHDOG_CAP_KIB} KiB cap (A3)"
  elif grep -q "timed out after" "$errfile"; then
    failure_note="driver exceeded the ${DARTQ_HARNESS_ORACLE_TIMEOUT_MS} ms budget and was killed (A11)"
  elif grep -q "exited with" "$errfile"; then
    failure_note="dartq exited outside the valid 0-3 analysis tiers — crash or panic (A10)"
  else
    return 1
  fi
}

# Run one bench face for scale $1 with metric $2 (extra args forwarded after
# --out, keeping the harness argument order). Non-strict: a bench failure is
# an environment error (die 2), today's shape. Strict: stderr is captured
# and classified — a driver-level failure is recorded as a breach and the
# ladder continues (per-scale reports stay independent), a harness-own
# failure dies with exit 2 and no verdict scored.
run_bench() {
  s="$1"
  metric="$2"
  shift 2
  if [ "$strict" -eq 0 ]; then
    "$HARNESS" bench --metric "$metric" --corpus-dir "tests/corpus/mega/$s" --out "$outdir" \
      "$@" --dartq-cmd "$DARTQ_CMD" ||
      die "bench $metric failed for $s (harness error; see its stderr)"
    return 0
  fi
  if "$HARNESS" bench --metric "$metric" --corpus-dir "tests/corpus/mega/$s" --out "$outdir" \
    "$@" --dartq-cmd "$DARTQ_CMD" 2>"$errfile"; then
    return 0
  fi
  if ! classify_bench_failure; then
    cat "$errfile" >&2
    die "harness-own failure during $metric for $s (no verdict scored)"
  fi
  strict_fail=1
  breaches="${breaches}[$label/$s] $metric: $failure_note
"
  return 1
}

# Strict acceptance rows for one scale,
# printed inside the summary. Reads the summary globals; appends breaches
# and sets strict_fail. A12 (corpus identity) runs on every scale and is an
# environment-tier failure when violated — the tree must come from the
# pinned generator contract or no measurement is valid. Breached budget rows
# mark the verdict FAIL without stopping the ladder.
strict_rows() {
  fp="$(jq -r '.fingerprint // ""' "tests/corpus/mega/$s/manifest.json" 2>/dev/null)" || fp=""
  seed="$(jq -r '.seed // ""' "tests/corpus/mega/$s/manifest.json" 2>/dev/null)" || seed=""
  if [ -z "$fp" ] || [ "$seed" != "0" ]; then
    die "corpus $s identity check failed (A12): fingerprint='$fp' seed='$seed' — regenerate with corpus-gen --seed 0"
  fi
  printf '  %-16s %s (seed %s) — echoed into the run record (A12)\n' "corpus_id" "$fp" "$seed"

  if [ "$rss_ok" -eq 1 ]; then
    if [ "$s" = "100k" ]; then
      a1="$(jq -r '.rss_per_100k_loc_kib | ceil' "$outdir/rss-100k/$s/report.json" 2>/dev/null)" || a1=""
      if [ -z "$a1" ]; then
        die "rss report for $s carries no rss_per_100k_loc_kib"
      fi
      if [ "$a1" -le "$rss_limit_kib" ]; then
        printf '  %-16s %s KiB <= %s KiB (the RSS charter row, decimal) — PASS\n' "A1 rss/100k" "$a1" "$rss_limit_kib"
      else
        printf '  %-16s %s KiB > %s KiB (the RSS charter row, decimal) — FAIL\n' "A1 rss/100k" "$a1" "$rss_limit_kib"
        strict_fail=1
        breaches="${breaches}[$label/$s] A1: rss_per_100k_loc_kib $a1 > $rss_limit_kib KiB
"
      fi
    else
      printf '  %-16s %s KiB — report-only (A2: fixed overhead dominates below the stress tier)\n' "A2 rss/100k" "$per_100k"
    fi
    printf '  %-16s peak %s KiB within the %s KiB cap, watchdog armed (runbook:8) — PASS\n' \
      "A3 watchdog" "$peak_rss" "$WATCHDOG_CAP_KIB"
  fi

  if [ "$inc_ok" -eq 1 ]; then
    case "$s" in
    1k)
      bar="$inc_p95_1k_ms"
      face="A4"
      cite="adopted #733: the resident-2s charter row analog at the 52k-LOC tier"
      ;;
    10k)
      bar="$inc_p95_10k_ms"
      face="A5"
      cite="adopted #733: 2x linear from the pinned 250k point"
      ;;
    100k)
      bar="$inc_p95_100k_ms"
      face="A6"
      cite="adopted #733: two-stage, r1 records the baseline"
      ;;
    esac
    p95i="$(jq -r '.p95_ms | ceil' "$outdir/incremental-p95/report.json" 2>/dev/null)" || p95i=""
    if [ -z "$p95i" ]; then
      die "incremental report for $s carries no p95_ms"
    fi
    if [ "$p95i" -le "$bar" ]; then
      printf '  %-16s p95 %s ms <= %s ms batch proxy (%s) — PASS\n' "$face inc_p95" "$p95i" "$bar" "$cite"
    else
      printf '  %-16s p95 %s ms > %s ms batch proxy (%s) — FAIL\n' "$face inc_p95" "$p95i" "$bar" "$cite"
      strict_fail=1
      breaches="${breaches}[$label/$s] $face: incremental p95 $p95i > $bar ms (batch proxy)
"
    fi
    printf '  %-16s baseline %sms + %s steps inside the %s ms driver budget — PASS\n' \
      "A11 completion" "$baseline" "$steps" "${DARTQ_HARNESS_ORACLE_TIMEOUT_MS}"
  fi

  if [ "$rss_ok" -eq 1 ] && [ "$inc_ok" -eq 1 ]; then
    printf '  %-16s both benches completed; every dartq exit within the valid 0-3 tiers — PASS\n' \
      "A10 zero-panic"
  fi
}

# --- oracle leg helpers (mega T2 fast tier + A9; run path only) -------------------------------------------------

# Run one differential face of the leg and classify the harness exit into
# $oracle_face_class: "ok" (harness exit 0 — the ERROR gate met the bar),
# "below-bar" (exit 1 — a measured result, never an environment error),
# "seeded-vacuous" (exit 2 carrying the harness's vacuity marker on the
# seeded face — the injections never reached the oracle: a leg failure,
# not an environment error). Any other exit 2 is harness-own and dies as
# an environment error with no verdict scored. Harness stderr is captured
# in both modes so the classification can read it, then passed through
# verbatim to keep the console parity a direct invocation would have.
run_oracle_face() {
  face="$1"
  dir="$2"
  shift 2
  odir="$outdir/oracle-$face"
  "$HARNESS" run --corpus-dir "$dir" --oracle-dart "$dart_bin" --dartq-cmd "$DARTQ_CMD" \
    --threshold "$oracle_jac_min" --out "$odir" "$@" 2>"$errfile"
  rc=$?
  cat "$errfile" >&2
  if [ "$rc" -eq 0 ]; then
    oracle_face_class="ok"
  elif [ "$rc" -eq 1 ]; then
    oracle_face_class="below-bar"
  elif [ "$rc" -eq 2 ] && [ "$face" = "seeded" ] && grep -q "vacuous" "$errfile"; then
    oracle_face_class="seeded-vacuous"
  else
    die "harness-own failure during the oracle-$face leg (no verdict scored)"
  fi
}

# One A9 summary row for a face: the (code,file) Jaccard row out of the
# harness report (same jq shape as m1-gate.sh). Under --strict a class
# other than "ok" records a breach and fails the verdict — the A9 assert
# (it asserts from the start; a FAIL is a legal r1 intermediate
# result, recorded with its attribution, not an environment error). Sets
# $oracle_row_ok=1 when a report row was actually read.
oracle_row() {
  face="$1"
  class="$2"
  oracle_row_ok=0
  rj="$outdir/oracle-$face/report.json"
  row="$(jq -r '[(.error_jaccard * 1000 | round / 1000 | tostring),
                 (.oracle_total | tostring),
                 (.dartq_total | tostring),
                 (.oracle_only | length | tostring),
                 (.dartq_only | length | tostring)] | @tsv' "$rj" 2>/dev/null)" || row=""
  if [ -z "$row" ]; then
    if [ "$class" = "ok" ]; then
      die "oracle-$face report $rj is missing or unreadable"
    fi
    printf '  %-16s no report (harness class: %s) — attribution in the console above\n' \
      "A9 $face" "$class"
    return
  fi
  oracle_row_ok=1
  IFS="$(printf '\t')" read -r jac ototal dtotal oonly donly <<EOF
$row
EOF
  if [ "$class" = "ok" ]; then
    printf '  %-16s jaccard %s | oracle %s vs dartq %s diags (%s/%s diff entries) — PASS\n' \
      "A9 $face" "$jac" "$ototal" "$dtotal" "$oonly" "$donly"
  else
    printf '  %-16s jaccard %s < %s | oracle %s vs dartq %s diags (%s/%s diff entries) — %s\n' \
      "A9 $face" "$jac" "$oracle_jac_min" "$ototal" "$dtotal" "$oonly" "$donly" \
      "$([ "$strict" -eq 1 ] && printf FAIL || printf record)"
    if [ "$strict" -eq 1 ]; then
      strict_fail=1
      breaches="${breaches}[$label/1k] A9 $face: ERROR (code,file) Jaccard $jac < $oracle_jac_min (attribution: $outdir/oracle-$face/report.json)
"
    fi
  fi
}

# The whole T2 leg for the 1k scale: seed the variant tree from the
# checked-in manifest, then the clean face (record face — the ruled clean
# regime passes --allow-vacuous so an empty oracle side is recorded, never
# fatal) and the seeded face (judgment face — vacuity is refused there).
# Stores both outcome classes; the A9 rows print in the per-scale summary
# (oracle_rows).
oracle_leg_run() {
  echo "-- [$label/1k] oracle leg: seeding $SEED_DIR from the checked-in manifest"
  bash "$SEED_SCRIPT" --src tests/corpus/mega/1k --dst "$SEED_DIR" --manifest "$SEED_MANIFEST" ||
    die "seeding $SEED_DIR failed (environment error)"
  echo "-- [$label/1k] oracle leg: clean-tree differential (record face + false-positive detector)"
  run_oracle_face clean tests/corpus/mega/1k --allow-vacuous
  clean_class="$oracle_face_class"
  echo "-- [$label/1k] oracle leg: seeded-tree differential (judgment face)"
  run_oracle_face seeded "$SEED_DIR"
  seeded_class="$oracle_face_class"
}

# The A7 cold leg for the 1k scale:
# the generalized oracle_baseline.sh driven twice over the same clean tree —
# the oracle side (oracle-of-record dart, `dart analyze --format=machine`,
# the instrument's default shape passed via --cmd so both legs share one
# JSON schema) and the dartq side (DARTQ_CMD, the m1-gate shape). Each leg
# prints one JSONL stream (round 1 = cold, rounds 2..N = hot) captured
# verbatim at <outdir>/cold/<side>.jsonl; the two summary records are the
# apples-to-apples cold pair (same env descriptor, diag_lines = the
# command's stdout line count on both sides). Runs FIRST in the per-scale
# flow — the coldest session state the 1k tree gets — and strictly
# sequentially: the single-instance discipline (never alongside another
# analysis process) is the instrument's own contract. A 30 s canary of the
# exact oracle shape precedes the sides (#1246), each side runs under the
# COLD_WALL_CAP_S wall cap (oracle_baseline.sh has no timeout of its own):
# a hung round is group-killed and dies loudly, never silently, and the
# instrument's WORK dir is pinned under $cdir/work so a cap-killed round's
# raw outputs survive for post-mortem.

# Wall-capped runner: the child
# sits in its own session (perl setsid — bash 3.2 has no setsid) so a
# breach group-kills exactly the analysis tree (/usr/bin/time + dart),
# never this runner. Returns 124 on a cap kill, the child's exit code
# otherwise; the structured kill line goes to stderr (the caller's stdout
# is the leg's JSONL stream). The nested session makes the leg briefly
# invisible to an outer serial-gates RSS poll — bounded by this cap, and
# the leg's calibrated footprint (~200 MB oracle side) sits orders below
# the chain caps.
capped_run() { # <cap_s> <cmd...>
  cr_cap="$1"
  shift
  cr_t0=$(date +%s)
  perl -MPOSIX -e 'setsid; exec @ARGV' -- "$@" &
  cr_pid=$!
  while kill -0 "$cr_pid" 2>/dev/null; do
    if [ $(( $(date +%s) - cr_t0 )) -ge "$cr_cap" ]; then
      cr_pgid="$(ps -o pgid= -p "$cr_pid" 2>/dev/null | tr -d ' ')"
      if [ -n "$cr_pgid" ]; then
        kill -9 -"$cr_pgid" 2>/dev/null
        sleep 0.2
        kill -9 -"$cr_pgid" 2>/dev/null
      fi
      printf 'mega-run: WALL_CAP_KILL cap=%ss elapsed=%ss\n' "$cr_cap" "$(( $(date +%s) - cr_t0 ))" >&2
      wait "$cr_pid" 2>/dev/null
      return 124
    fi
    sleep 5
  done
  wait "$cr_pid" 2>/dev/null
}

# Canary preflight for the A7 cold leg (#1246): run the EXACT oracle command
# shape once — same capped_run session nesting, same /usr/bin/time wrapper,
# same resolved dart binary — against a bare one-file corpus (no pubspec, so
# no implicit pub get; dart analyze works context-free) under a 30 s cap. A
# hung oracle then fails in 30 s with captured evidence (canary.log plus the
# pinned WORK files under $cdir/work/) instead of burning the side cap on
# round 1. The rounds:1 summary line in canary.log carries empty hot fields
# (an oracle_baseline.sh single-round quirk) — the log is diagnostic; the
# leg jsonl files are the records.
cold_canary_run() {
  mkdir -p "$cdir/canary" || die "creating $cdir/canary failed"
 printf 'void main {}\n' >"$cdir/canary/main.dart"
  echo "-- [$label/1k] A7 cold leg: oracle canary (${COLD_CANARY_CAP_S}s bound)"
  capped_run "$COLD_CANARY_CAP_S" bash tests/bench/oracle_baseline.sh "$cdir/canary" 1 \
    --cmd "$dart_bin analyze --format=machine" >"$cdir/canary.log" 2>&1
  canary_rc=$?
  [ "$canary_rc" -eq 0 ] && return 0
  if [ "$canary_rc" -eq 124 ]; then
    die "A7 cold leg: oracle canary hung — killed at the ${COLD_CANARY_CAP_S}s wall cap; the measured leg would hang the same way (see $cdir/canary.log and the surviving out.1/err.1/time.1 under $cdir/work/)"
  fi
  die "A7 cold leg: oracle canary failed (exit $canary_rc, see $cdir/canary.log)"
}

# One side of the A7 cold leg, wall-capped: <side> is "oracle" (the
# oracle-of-record dart, the instrument's default shape) or "dartq"
# (DARTQ_CMD, the m1-gate shape). A cap kill or a failed leg dies naming
# the side and how far the rounds got — a red step, never a silent skip.
cold_side() {
  side="$1"
  case "$side" in
  oracle) side_cmd="$dart_bin analyze --format=machine";;
  dartq) side_cmd="$DARTQ_CMD";;
  esac
  echo "-- [$label/1k] A7 cold leg: $side side ($COLD_ROUNDS rounds, round 1 = cold)"
  capped_run "$COLD_WALL_CAP_S" bash tests/bench/oracle_baseline.sh "tests/corpus/mega/1k" "$COLD_ROUNDS" \
    --cmd "$side_cmd" >"$cdir/$side.jsonl"
  side_rc=$?
  [ "$side_rc" -eq 0 ] && return 0
  done_rounds="$(grep -c '"record":"round"' "$cdir/$side.jsonl")" || done_rounds=0
  if [ "$side_rc" -eq 124 ]; then
    die "A7 cold leg: $side side hung — killed at the ${COLD_WALL_CAP_S}s wall cap after $done_rounds/$COLD_ROUNDS rounds (see $cdir/$side.jsonl; the in-flight round's out.N/err.N/time.N survive under $cdir/work/)"
  fi
  die "$side cold leg failed (exit $side_rc, see $cdir/$side.jsonl)"
}

cold_leg_run() {
  cdir="$outdir/cold"
  mkdir -p "$cdir" "$cdir/work" || die "creating $cdir failed"
  # Pin the instrument's WORK dir inside the evidence dir: oracle_baseline.sh
  # creates its WORK via `mktemp -d`, which honors TMPDIR, so a cap-killed
  # round's out.N/err.N/time.N survive under $cdir for post-mortem instead
  # of a trap-less SIGKILL stranding them in the system temp (#1246 — the r1
  # post-mortem only succeeded because a stray system WORK dir happened to
  # survive). Restored after the leg so the harness benches keep the ambient
  # temp.
  TMPDIR_SAVE="${TMPDIR:-}"
  export TMPDIR="$cdir/work"
  cold_canary_run
  cold_side oracle
  cold_side dartq
  if [ -n "$TMPDIR_SAVE" ]; then
    export TMPDIR="$TMPDIR_SAVE"
  else
    unset TMPDIR
  fi
  for side in oracle dartq; do
    grep -q '"record":"summary"' "$cdir/$side.jsonl" ||
      die "$cdir/$side.jsonl carries no summary record (leg incomplete)"
  done
}

# One A7 summary row for the cold pair: the round-1 (cold) wall times off
# both legs' summary lines plus the oracle/dartq ratio. Record-only — the
# assert (dartq <= 1/2 oracle, i.e. ratio >= 2) is the acceptance ruling's
# call, not this run's.
cold_rows() {
  oms="$(sed -nE 's/.*"cold_real_ms":([0-9]+).*/\1/p' "$outdir/cold/oracle.jsonl" 2>/dev/null | head -n1)"
  dms="$(sed -nE 's/.*"cold_real_ms":([0-9]+).*/\1/p' "$outdir/cold/dartq.jsonl" 2>/dev/null | head -n1)"
  if [ -z "$oms" ] || [ -z "$dms" ]; then
    printf '  %-16s no cold summaries under %s/cold/ — leg incomplete\n' "A7 cold" "$outdir"
    return
  fi
  if [ "$dms" -eq 0 ]; then
    printf '  %-16s oracle %sms vs dartq 0ms — unusable dartq cold round (see its exit_code in the jsonl)\n' \
      "A7 cold" "$oms"
    return
  fi
  ratio=$(awk -v o="$oms" -v d="$dms" 'BEGIN { printf "%.2f", o / d }')
  printf '  %-16s oracle %sms vs dartq %sms cold (round 1 of %s; oracle/dartq %s; bar >= 2.00) — record\n' \
    "A7 cold" "$oms" "$dms" "$COLD_ROUNDS" "$ratio"
}

# A9 rows for the per-scale summary (asserted under --strict, record-only
# otherwise). A vacuous seeded face is a breach in both wordings' terms —
# the checked-in injections never reached the oracle.
oracle_rows() {
  oracle_row clean "$clean_class"
  if [ "$seeded_class" = "seeded-vacuous" ]; then
    printf '  %-16s oracle side empty post-injection — the manifest injections never reached the oracle\n' "A9 seeded"
    if [ "$strict" -eq 1 ]; then
      strict_fail=1
      breaches="${breaches}[$label/1k] A9 seeded: vacuous oracle side — the checked-in injections never reached the oracle (manifest/injector bug)
"
    fi
  else
    oracle_row seeded "$seeded_class"
  fi
  cold_rows
}

# --- plan --------------------------------------------------------------------
# Refuse-overwrite guard (m2-gate precedent): --scale, --all and --oracle-leg
# all write their evidence under the same label root, so one check here covers
# every entry path. Refuses in dry-run mode too, mirroring m2-gate.
if [ -e "$OUT_ROOT/$label" ]; then
  die "$OUT_ROOT/$label already exists — refusing to overwrite recorded gate evidence (pass a new --label)"
fi
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
if [ "$oracle_leg" -eq 1 ]; then
  if [ -d "$SEED_DIR" ]; then
    seeded_state="present (refreshed from scratch on the run path)"
  else
    seeded_state="absent -> will seed from tests/corpus/mega/1k"
  fi
  printf '  %-16s %s (%s)\n' "scale 1k-seeded" "$SEED_DIR" "$seeded_state"
fi
printf '  %-16s %s\n' "config" ".dart_tool/package_config.json materialized per scale if absent (harness preflight)"
printf '  %-16s two bench faces per scale: rss-100k + incremental-p95 (edit-manifest.json written into the report dir)\n' "metrics"
if [ "$oracle_leg" -eq 1 ]; then
  printf '  %-16s 1k T2 oracle differential leg: harness run on the clean tree (record face, --allow-vacuous) + the seeded tree (judgment face)\n' "oracle"
  printf '  %-16s %s (seeded from %s via %s)\n' "seeded tree" "$SEED_DIR" "$SEED_MANIFEST" "$SEED_SCRIPT"
  printf '  %-16s ERROR (code,file) Jaccard >= %s on both faces — harness exit 0 (A9; bar override MEGA_ORACLE_JACCARD_MIN)\n' "A9 bar" "$oracle_jac_min"
  printf '  %-16s A7 cold pair: oracle_baseline.sh x2 on the 1k tree (%s rounds each, round 1 = cold, %ss oracle canary, %ss wall cap per side) -> cold/{oracle,dartq}.jsonl\n' "cold leg" "$COLD_ROUNDS" "$COLD_CANARY_CAP_S" "$COLD_WALL_CAP_S"
  printf '  %-16s %s (oracle-of-record; version mismatch = WARN, report meta records the real banner)\n' "oracle dart" "$dart_bin"
else
  printf '  %-16s NO oracle differential (S4 is a dartq-side budget)\n' "oracle"
fi
printf '  %-16s target/evidence/mega/%s/<scale>/\n' "reports" "$label"
if [ "$strict" -eq 1 ]; then
  echo "== strict acceptance gate"
  printf '  %-6s %s\n' "A1" "rss_per_100k_loc_kib <= $rss_limit_kib KiB at 100k (RSS charter row, decimal; 1k/10k report-only per A2)"
  printf '  %-6s %s\n' "A3" "watchdog tree cap $WATCHDOG_CAP_KIB KiB — harness breach = FAIL (runbook:8)"
  printf '  %-6s %s\n' "A4" "incremental p95 batch proxy <= $inc_p95_1k_ms ms at 1k (adopted #733: the resident-2s charter row analog)"
  printf '  %-6s %s\n' "A5" "incremental p95 batch proxy <= $inc_p95_10k_ms ms at 10k (adopted #733: 2x linear from the 250k point)"
  printf '  %-6s %s\n' "A6" "incremental p95 batch proxy <= $inc_p95_100k_ms ms at 100k (adopted #733: two-stage r1 record / r2 assert)"
  printf '  %-6s %s\n' "A10" "zero panic — dartq exit outside the valid 0-3 tiers = FAIL (the zero-panic charter row)"
  printf '  %-6s %s\n' "A11" "completion — driver timeout at ${DARTQ_HARNESS_ORACLE_TIMEOUT_MS} ms = FAIL"
  printf '  %-6s %s\n' "A12" "manifest fingerprint echoed + seed-0 corpus identity checked"
  if [ "$oracle_leg" -eq 1 ]; then
    printf '  %-6s %s\n' "A9" "ERROR (code,file) Jaccard >= $oracle_jac_min at 1k — clean face (record + false-positive detector) + seeded face (judgment)"
    printf '  %-6s %s\n' "A7" "cold pair measured, record-only (oracle-of-record dart vs DARTQ_CMD, $COLD_ROUNDS rounds each, round 1 = cold; assert bar dartq <= 1/2 oracle not asserted this round)"
    printf '  %-6s %s\n' "SKIP" "A8 10k/100k oracle DNF record — stress tiers, arbitration = A1 (the stress-tier arbitration row)"
  else
    printf '  %-6s %s\n' "SKIP" "A7/A8/A9 oracle differential faces — mega T2 (deferred face)"
  fi
  printf '  %-6s %s\n' "SKIP" "A13 S4 LOC accounting — parked T4 (bench non-blank vs manifest 5402483 lines)"
  printf '  %-6s %s\n' "env" "bar overrides: MEGA_RSS_100K_LIMIT_KIB MEGA_INC_P95_1K_MS MEGA_INC_P95_10K_MS MEGA_INC_P95_100K_MS$([ "$oracle_leg" -eq 1 ] && printf ' MEGA_ORACLE_JACCARD_MIN')"
fi
if [ "$dry_run" = 1 ]; then
  echo "== dry run: plan resolved, nothing executed"
  exit 0
fi

# --- run: strict verdict accumulators (the ladder always runs to its end —
# per-scale reports stay independent — and the final verdict aggregates
# every breach) ---------------------------------------------------------------
strict_fail=0
breaches=""
errfile=""
if [ "$strict" -eq 1 ] || [ "$oracle_leg" -eq 1 ]; then
  errfile="$(mktemp "${TMPDIR:-/tmp}/mega-run.XXXXXX")" || die "mktemp failed"
  trap 'rm -f "$errfile"' EXIT
fi

# Oracle-of-record banner check (run path only; mirrors m1-gate.sh — the
# report meta records the real version either way, a mismatch is a WARN).
if [ "$oracle_leg" -eq 1 ]; then
  found_ver="$("$dart_bin" --version 2>&1 | sed -nE 's/^Dart SDK version: ([^ ]+).*/\1/p')"
  if [ "$found_ver" != "$EXPECTED_ORACLE_VERSION" ]; then
    echo "mega-run: WARN oracle dart expected $EXPECTED_ORACLE_VERSION, found '${found_ver:-unrecognized banner}'"
  fi
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
  rss_ok=1
  inc_ok=1
  # A7 cold pair first: the coldest session state the tree gets, and the
  # single-instance discipline keeps the numbers clean (sequential rounds).
  if [ "$oracle_leg" -eq 1 ] && [ "$s" = "1k" ]; then
    cold_leg_run
  fi
  run_bench "$s" rss-100k || rss_ok=0
  run_bench "$s" incremental-p95 --manifest "$outdir/edit-manifest.json" || inc_ok=0
  clean_class=""
  seeded_class=""
  if [ "$oracle_leg" -eq 1 ] && [ "$s" = "1k" ]; then
    oracle_leg_run
  fi
  wall=$((SECONDS - started))

  # --- summary ---------------------------------------------------------------
  rss=""
  inc=""
  if [ "$rss_ok" -eq 1 ]; then
    rss="$(jq -r '[.file_count, .loc, .peak_rss_kib, .rss_per_100k_loc_kib, .samples] | @tsv' \
      "$outdir/rss-100k/$s/report.json" 2>/dev/null)" || rss=""
  fi
  if [ "$inc_ok" -eq 1 ]; then
    inc="$(jq -r '[.baseline_duration_ms, .p50_ms, .p95_ms, .p99_ms, (.steps | length)] | @tsv' \
      "$outdir/incremental-p95/report.json" 2>/dev/null)" || inc=""
  fi
  if [ "$strict" -eq 0 ]; then
    if [ -z "$rss" ] || [ -z "$inc" ]; then
      die "report(s) under $outdir are missing or unreadable"
    fi
  else
    # A failed face already recorded its breach; a report missing after a
    # successful bench is an environment error.
    if [ "$rss_ok" -eq 1 ] && [ -z "$rss" ]; then
      die "report $outdir/rss-100k/$s/report.json is missing or unreadable"
    fi
    if [ "$inc_ok" -eq 1 ] && [ -z "$inc" ]; then
      die "report $outdir/incremental-p95/report.json is missing or unreadable"
    fi
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
  if [ -n "$rss" ]; then
    printf '  %-16s %s files, %s LOC\n' "corpus" "$files" "$loc"
    printf '  %-16s %s KiB peak (%s samples) = %s KiB per 100k LOC\n' "rss_100k" "$peak_rss" "$samples" "$per_100k"
  elif [ "$strict" -eq 1 ]; then
    printf '  %-16s FAILED — breach recorded, see the final verdict\n' "rss_100k"
  fi
  if [ -n "$inc" ]; then
    printf '  %-16s baseline %sms | %s steps: p50 %sms p95 %sms p99 %sms\n' \
      "incremental" "$baseline" "$steps" "$p50" "$p95" "$p99"
  elif [ "$strict" -eq 1 ]; then
    printf '  %-16s FAILED — breach recorded, see the final verdict\n' "incremental"
  fi
  if [ "$strict" -eq 1 ]; then
    strict_rows
  else
    printf '  %-16s pending — thresholds assert in the acceptance run once a baseline exists\n' "budget_check"
  fi
  if [ "$oracle_leg" -eq 1 ] && [ "$s" = "1k" ]; then
    oracle_rows
  fi
  echo "  reports: $outdir/rss-100k/$s/report.json + $outdir/incremental-p95/report.json"
  if [ "$oracle_leg" -eq 1 ] && [ "$s" = "1k" ]; then
    printf '  %-16s %s\n' "oracle reports" "$outdir/{oracle-clean,oracle-seeded}/report.json (+ raw-outputs.json: both sides, unfiltered — the oracle-face record)"
    printf '  %-16s %s\n' "cold reports" "$outdir/cold/{oracle,dartq}.jsonl (round+summary records, identical schema)"
  fi
done

echo
if [ "$oracle_leg" -eq 1 ]; then
  deferred="deferred faces: A8 stress-tier DNF record (arbitration = A1; A7 cold pair is measured above, record-only); A13 LOC accounting -> parked T4"
else
  deferred="deferred faces: A7/A8/A9 oracle leg -> mega T2; A13 LOC accounting -> parked T4 (deferred face)"
fi
if [ "$strict" -eq 1 ]; then
  if [ "$strict_fail" -eq 1 ]; then
    echo "mega-run: VERDICT FAIL (strict acceptance gate breached — label $label)"
    printf '%s\n' "$breaches" | while IFS= read -r b; do
      if [ -n "$b" ]; then printf '  %s\n' "$b"; fi
    done
    echo "  $deferred"
    exit 1
  fi
  echo "mega-run: VERDICT PASS (strict acceptance gate met — label $label)"
  echo "  $deferred"
  exit 0
fi
echo "mega-run: VERDICT COMPLETE (no thresholds asserted; exit 1 reserved for the acceptance run)"
exit 0
