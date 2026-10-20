#!/usr/bin/env bash
# real-corpus-sweep.sh — real-world pub ecosystem differential + perf sweep
# over tests/corpus/real/ (ticket #1136; protocol: tests/corpus/real/README.md).
#
# Three faces, per package (sources vendored from the local pub cache, see
# each package's PROVENANCE.md):
#   1. resolution — materialize <pkg>/.dart_tool/ when absent:
#      `dart pub get --offline` first; when the local cache cannot resolve
#      the dependency set (e.g. the plugin packages' `sdk: flutter`), a
#      hand-written self-referential package_config.json that also exposes
#      the four vendored zero-dep corpus packages (args/collection/meta/
#      path) as siblings, plus a minimal pubspec.lock. Every entry carries
#      `packageUri: lib/` (pub's reader contract — entries without it
#      anchor `package:` imports at the package root, breaking them).
#      The mode is recorded to $OUT/resolution.tsv. Existing configs are
#      kept only when they carry `packageUri`; stale ones (pre-packageUri
#      fallback configs) are re-materialized.
#   2. diff — `harness run` per package: oracle `dart analyze` vs
#      `dartq analyze --format=machine`, all severities judged, divergences
#      persisted in the standard regression-case-v1 format under
#      $OUT/cases/<pkg>/. No bespoke diff logic — the harness owns the
#      (code, file) caliber.
#   3. perf — `harness bench --metric cold --metric rss-100k` per package:
#      one cold full-analysis wall time per side (+ ratio) and peak
#      tree-RSS via the bench's ps-polling sampler (watchdog attached).
#
# A final pass folds every per-package report into $OUT/summary.json and
# summary.md (per-package jaccards, diff rows, resolution mode, cold times,
# peak RSS, totals).
#
# Divergences are discoveries, not sweep failures: they are persisted as
# regression cases and surfaced in the summary; the sweep still exits 0.
# Exit 2 = preflight or harness/infra error on any face.
#
# SERIAL PHASE ONLY: every face runs real dart/dartq/harness processes
# (memory-incident discipline). Never alongside development lanes.
#
# Usage: scripts/real-corpus-sweep.sh [--label L] [--dry-run] [--skip-perf]
#   --label L    evidence dir name under target/evidence/real-sweep/
#                (default REAL_SWEEP_LABEL env, else real-r1). An existing
#                directory refuses to run — recorded evidence is never
#                overwritten (bump the label).
#   --dry-run    print the planned sequence per package and exit; runs no
#                dart/cargo/dartq process. DARTQ_DART_SDK may be unset (the
#                commands echo a placeholder).
#   --skip-perf  diff face only (useful when re-sweeping divergences).
# Env:   DARTQ_DART_SDK  pinned Dart SDK root (>= 3.13), required unless
#                        --dry-run (same contract as scripts/plugin-ab.sh).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

CORPUS="tests/corpus/real"
LABEL="${REAL_SWEEP_LABEL:-real-r1}"
DRY_RUN=0
SKIP_PERF=0

usage() { sed -n '2,40p' "$0" | grep -E '^#( |$)' | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --skip-perf) SKIP_PERF=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "real-corpus-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

OUT="target/evidence/real-sweep/$LABEL"
DARTQ_BIN="./target/release/dartq"
HARNESS_BIN="./target/release/harness"
SDK_DESC="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
DARTQ_CMD="$DARTQ_BIN analyze --format=machine --sdk $SDK_DESC"

# --- package discovery --------------------------------------------------------
# One package per directory carrying a PROVENANCE.md (the vendored-copy mark).
PACKAGES=()
for dir in "$CORPUS"/*/; do
  [ -f "$dir/PROVENANCE.md" ] && PACKAGES+=("$(basename "$dir")")
done
if [ "${#PACKAGES[@]}" -eq 0 ]; then
  echo "real-corpus-sweep: no packages under $CORPUS (expected one dir + PROVENANCE.md per package)" >&2
  exit 2
fi

echo "real-corpus-sweep: label=$LABEL packages=${#PACKAGES[@]}: ${PACKAGES[*]}"
echo "real-corpus-sweep: evidence -> $OUT"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "real-corpus-sweep: DRY RUN — planning only, no process is executed"
  for pkg in "${PACKAGES[@]}"; do
    dir="$CORPUS/$pkg"
    config="$dir/.dart_tool/package_config.json"
    if [ -f "$config" ] && grep -q '"packageUri"' "$config"; then
      echo "  [$pkg] resolution: keep existing .dart_tool/package_config.json"
    elif [ -f "$config" ]; then
      echo "  [$pkg] resolution: re-materialize (config lacks packageUri): dart pub get --offline, on failure hand-written fallback config (+ minimal pubspec.lock)"
    else
      echo "  [$pkg] resolution: dart pub get --offline, on failure hand-written fallback config (+ minimal pubspec.lock)"
    fi
    echo "  [$pkg] diff: $HARNESS_BIN run --corpus-dir $dir --oracle-dart $SDK_DESC/bin/dart --dartq-cmd \"$DARTQ_CMD\" --severity-filter all --allow-vacuous --persist-regressions $OUT/cases/$pkg --out $OUT/diff/$pkg"
    if [ "$SKIP_PERF" -eq 0 ]; then
      echo "  [$pkg] perf: $HARNESS_BIN bench --metric cold --metric rss-100k --corpus-dir $dir --dartq-cmd \"$DARTQ_CMD\" --oracle-dart $SDK_DESC/bin/dart --sdk $SDK_DESC --out $OUT/bench/$pkg"
    fi
  done
  echo "  [aggregate] fold $OUT/diff/*/report.json + $OUT/bench/* into $OUT/summary.json, summary.md"
  exit 0
fi

# --- preflight (real runs only) -----------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "real-corpus-sweep: DARTQ_DART_SDK is required (pinned Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "real-corpus-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
if [ ! -x "$HARNESS_BIN" ]; then
  echo "real-corpus-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$DARTQ_BIN" ]; then
  echo "real-corpus-sweep: $DARTQ_BIN missing after build" >&2
  exit 2
fi
if [ -d "$OUT" ]; then
  echo "real-corpus-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"
RESOLUTION="$OUT/resolution.tsv"
printf 'package\tmode\tnote\n' >"$RESOLUTION"

# --- face 1: resolution --------------------------------------------------------
# Hand-written fallback: the package itself (languageVersion 3.13 = the
# harness preflight floor, overridden from the upstream bound like the
# legacy packages) plus the vendored zero-dep corpus packages as siblings,
# so their imports resolve offline and the cross-library face is exercised.
# Every entry pins `packageUri: lib/` — pub always emits it; a config
# without it resolves `package:` imports to the package root on both sides.
write_fallback_config() {
  # Two statements: expanding $pkg in the same `local` list only works when
  # the caller happens to carry a same-named variable (set -u landmine).
  local pkg="$1"
  local dir="$CORPUS/$pkg"
  mkdir -p "$dir/.dart_tool"
  printf '{"configVersion": 2, "generator": "pub", "packages": [\n' >"$dir/.dart_tool/package_config.json"
  printf ' {"name": "%s", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.13"}' "$pkg" >>"$dir/.dart_tool/package_config.json"
  [ -d "$CORPUS/args" ] && printf ',\n {"name": "args", "rootUri": "../../args", "packageUri": "lib/", "languageVersion": "3.3"}' >>"$dir/.dart_tool/package_config.json"
  [ -d "$CORPUS/collection" ] && printf ',\n {"name": "collection", "rootUri": "../../collection", "packageUri": "lib/", "languageVersion": "3.4"}' >>"$dir/.dart_tool/package_config.json"
  [ -d "$CORPUS/meta" ] && printf ',\n {"name": "meta", "rootUri": "../../meta", "packageUri": "lib/", "languageVersion": "3.5"}' >>"$dir/.dart_tool/package_config.json"
  [ -d "$CORPUS/path" ] && printf ',\n {"name": "path", "rootUri": "../../path", "packageUri": "lib/", "languageVersion": "3.4"}' >>"$dir/.dart_tool/package_config.json"
  printf ']\n}\n' >>"$dir/.dart_tool/package_config.json"
  printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$dir/pubspec.lock"
}

echo "real-corpus-sweep: face 1 — resolution"
for pkg in "${PACKAGES[@]}"; do
  dir="$CORPUS/$pkg"
  config="$dir/.dart_tool/package_config.json"
  if [ -f "$config" ] && grep -q '"packageUri"' "$config"; then
    printf '%s\tkept\tpre-existing config\n' "$pkg" >>"$RESOLUTION"
    continue
  fi
  note="deps unresolvable offline; hand-written fallback (self + vendored zero-dep siblings)"
  if [ -f "$config" ]; then
    # A config without `packageUri` (pre-packageUri fallback output) breaks
    # every `package:` import on both sides — re-materialize, never keep.
    echo "real-corpus-sweep: [$pkg] config lacks packageUri — re-materializing" >&2
    note="stale config without packageUri; $note"
  fi
  if (cd "$dir" && dart pub get --offline >/dev/null 2>"$OUT/$pkg.pubget.err"); then
    printf '%s\tpub-offline\t%s\n' "$pkg" "dart pub get --offline resolved from the local cache" >>"$RESOLUTION"
  elif write_fallback_config "$pkg"; then
    printf '%s\tfallback\t%s\n' "$pkg" "$note" >>"$RESOLUTION"
  else
    printf '%s\tfailed\tcould not materialize resolution\n' "$pkg" >>"$RESOLUTION"
    echo "real-corpus-sweep: [$pkg] resolution failed (see $OUT/$pkg.pubget.err)" >&2
  fi
done
cat "$RESOLUTION"

# --- face 2: diff ---------------------------------------------------------------
# harness run exit codes: 0 pass (jaccard >= threshold), 1 divergences found
# (discovery — cases persisted), 2 harness/infra error (recorded, sweep
# continues; final exit reflects it).
echo "real-corpus-sweep: face 2 — differential (all severities)"
HARNESS_ERRORS=0
for pkg in "${PACKAGES[@]}"; do
  status=0
  "$HARNESS_BIN" run \
    --corpus-dir "$CORPUS/$pkg" \
    --oracle-dart "$DARTQ_DART_SDK/bin/dart" \
    --dartq-cmd "$DARTQ_CMD" \
    --severity-filter all \
    --allow-vacuous \
    --persist-regressions "$OUT/cases/$pkg" \
    --out "$OUT/diff/$pkg" >"$OUT/$pkg.diff.log" 2>&1 || status=$?
  if [ "$status" -eq 2 ]; then
    printf '%s\tdiff\tHARNESS ERROR (exit 2) — see %s\n' "$pkg" "$OUT/$pkg.diff.log" >>"$RESOLUTION"
    echo "real-corpus-sweep: [$pkg] diff face harness error — see $OUT/$pkg.diff.log" >&2
    HARNESS_ERRORS=1
  else
    echo "real-corpus-sweep: [$pkg] diff face done (exit $status; 1 = divergences persisted) — $OUT/$pkg.diff.log"
  fi
done

# --- face 3: perf ---------------------------------------------------------------
if [ "$SKIP_PERF" -eq 0 ]; then
  echo "real-corpus-sweep: face 3 — perf (cold wall time + peak RSS)"
  for pkg in "${PACKAGES[@]}"; do
    status=0
    "$HARNESS_BIN" bench \
      --metric cold --metric rss-100k \
      --corpus-dir "$CORPUS/$pkg" \
      --dartq-cmd "$DARTQ_CMD" \
      --oracle-dart "$DARTQ_DART_SDK/bin/dart" \
      --sdk "$DARTQ_DART_SDK" \
      --out "$OUT/bench/$pkg" >"$OUT/$pkg.bench.log" 2>&1 || status=$?
    if [ "$status" -ne 0 ]; then
      printf '%s\tbench\tFAILED (exit %s) — see %s\n' "$pkg" "$status" "$OUT/$pkg.bench.log" >>"$RESOLUTION"
      echo "real-corpus-sweep: [$pkg] bench failed (exit $status) — see $OUT/$pkg.bench.log" >&2
      HARNESS_ERRORS=1
    else
      echo "real-corpus-sweep: [$pkg] bench done — $OUT/bench/$pkg"
    fi
  done
fi

# --- aggregate -------------------------------------------------------------------
# Pure folding of the harness's own reports into one table — no diff logic
# here; the caliber lives entirely in the harness.
AGG="$OUT/aggregate.py"
cat >"$AGG" <<'PY'
import glob
import json
import os
import sys

out = sys.argv[1]


def load(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def fmt(v):
    if v is None:
        return ""
    if isinstance(v, float):
        return "%.3f" % v
    return str(v)


rows = []
for report_path in sorted(glob.glob(os.path.join(out, "diff", "*", "report.json"))):
    pkg = report_path.split(os.sep)[-2]
    rep = load(report_path)
    row = {
        "package": pkg,
        "file_count": rep.get("file_count"),
        "oracle_total": rep.get("oracle_total"),
        "dartq_total": rep.get("dartq_total"),
        "error_jaccard": rep.get("error_jaccard"),
        "passed": rep.get("passed"),
        "oracle_only_rows": len(rep.get("oracle_only") or []),
        "dartq_only_rows": len(rep.get("dartq_only") or []),
        "severities": {
            s["severity"]: s["jaccard"] for s in rep.get("severities", [])
        },
        "cases": sorted(
            os.path.basename(p)
            for p in glob.glob(os.path.join(out, "cases", pkg, "*.json"))
        ),
    }
    cold = os.path.join(out, "bench", pkg, "cold", "report.json")
    if os.path.exists(cold):
        c = load(cold)
        row["cold"] = {
            "loc": c.get("loc"),
            "dartq_ms": c.get("dartq_elapsed_ms"),
            "oracle_ms": c.get("oracle_elapsed_ms"),
            "ratio": c.get("cold_ratio"),
        }
    rss = os.path.join(out, "bench", pkg, "rss-100k", pkg, "report.json")
    if os.path.exists(rss):
        r = load(rss)
        row["rss"] = {
            "peak_rss_kib": r.get("peak_rss_kib"),
            "rss_per_100k_loc_kib": r.get("rss_per_100k_loc_kib"),
            "elapsed_ms": r.get("elapsed_ms"),
        }
    rows.append(row)

resolution = {}
res_tsv = os.path.join(out, "resolution.tsv")
if os.path.exists(res_tsv):
    with open(res_tsv, "r", encoding="utf-8") as fh:
        for line in fh.read().splitlines()[1:]:
            fields = line.split("\t")
            if len(fields) >= 2:
                resolution[fields[0]] = fields[1]
for row in rows:
    row["resolution"] = resolution.get(row["package"], "unknown")

summary = {"schema": "real-corpus-sweep-summary-v1", "packages": rows}
tot_o = sum(r["oracle_total"] or 0 for r in rows)
tot_d = sum(r["dartq_total"] or 0 for r in rows)
summary["totals"] = {
    "packages": len(rows),
    "oracle_instances": tot_o,
    "dartq_instances": tot_d,
    "oracle_only_rows": sum(r["oracle_only_rows"] for r in rows),
    "dartq_only_rows": sum(r["dartq_only_rows"] for r in rows),
    "cases": sum(len(r["cases"]) for r in rows),
}

with open(os.path.join(out, "summary.json"), "w", encoding="utf-8") as fh:
    json.dump(summary, fh, indent=2, ensure_ascii=False)
    fh.write("\n")

md = ["# Real-corpus sweep summary", "",
      "| package | resolution | files | oracle | dartq | ERROR jac | diff rows (o/d) | cases | cold dartq ms | cold oracle ms | peak RSS KiB |",
      "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
for r in rows:
    cold = r.get("cold") or {}
    rss = r.get("rss") or {}
    md.append(
        "| {pkg} | {res} | {files} | {o} | {d} | {ej} | {ro}/{rd} | {cases} | {cdq} | {cor} | {rssk} |".format(
            pkg=r["package"], res=r["resolution"], files=fmt(r["file_count"]),
            o=fmt(r["oracle_total"]), d=fmt(r["dartq_total"]),
            ej=fmt(r["error_jaccard"]), ro=r["oracle_only_rows"],
            rd=r["dartq_only_rows"], cases=len(r["cases"]),
            cdq=fmt(cold.get("dartq_ms")), cor=fmt(cold.get("oracle_ms")),
            rssk=fmt(rss.get("peak_rss_kib")),
        )
    )
t = summary["totals"]
md.append("| **totals** | | | {} | {} | | {}/{} | {} | | | |".format(
    t["oracle_instances"], t["dartq_instances"],
    t["oracle_only_rows"], t["dartq_only_rows"], t["cases"]))
md += ["", "Per-severity jaccards, sample rows, and cold/rss detail: see "
       "`summary.json` and each package's `diff/<pkg>/report.md`, "
       "`bench/<pkg>/{cold,rss-100k}/report.json`."]
with open(os.path.join(out, "summary.md"), "w", encoding="utf-8") as fh:
    fh.write("\n".join(md) + "\n")

print(
    f"aggregate: {t['packages']} packages, oracle={t['oracle_instances']} "
    f"dartq={t['dartq_instances']} diff rows o={t['oracle_only_rows']} "
    f"d={t['dartq_only_rows']} cases={t['cases']}"
)
PY

python3 "$AGG" "$OUT"

echo "real-corpus-sweep: complete — evidence in $OUT (summary.md, resolution.tsv, diff/, bench/, cases/)"
if [ "$HARNESS_ERRORS" -ne 0 ]; then
  echo "real-corpus-sweep: FAILED faces present — see resolution.tsv notes and per-package logs" >&2
  exit 2
fi
exit 0
