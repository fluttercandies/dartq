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
#      path) as siblings plus every direct dependency already resolvable
#      in the local pub cache (exact versions from pubspec.lock when it
#      carries them, else the best cached match for the declared
#      constraint), plus a minimal pubspec.lock when none exists. Every
#      entry carries `packageUri: lib/` (pub's reader contract — entries
#      without it anchor `package:` imports at the package root, breaking
#      them). The mode is recorded to $OUT/resolution.tsv. Existing
#      configs are kept only when they carry `packageUri`; stale ones
#      (pre-packageUri fallback configs) are re-materialized.
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
# Exit 2 = preflight or harness/infra error on any face, or a divergent
# per-side URI_DOES_NOT_EXIST count (a broken-resolution artifact: the two
# engines must fail to resolve, or resolve, the exact same tree).
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
# Per-side URI_DOES_NOT_EXIST counts (harness raw outputs, face 2). Equal
# counts are the pin that both engines read the same resolution graph; any
# divergence means one side is reading a different tree and fails the sweep.
URIMISS="$OUT/uri_miss.tsv"
printf 'package\turi_miss_dq\turi_miss_oracle\n' >"$URIMISS"

# --- face 1: resolution --------------------------------------------------------
# Hand-written fallback. Beyond the package itself (languageVersion 3.13 =
# the harness preflight floor, overridden from the upstream bound like the
# legacy packages) and the vendored zero-dep corpus packages as siblings,
# every direct dependency declared in the package's pubspec.yaml is
# resolved against the local pub cache: exact versions come from
# pubspec.lock's hosted entries when it carries them, otherwise the highest
# cached version satisfying the declared constraint wins (pub's own
# offline rule). Dependencies the cache cannot satisfy (sdk:/path:/git:,
# or no matching cached version) are left out — the tree stays exactly as
# resolvable as before this expansion. Every entry pins `packageUri: lib/`
# — pub always emits it; a config without it resolves `package:` imports
# to the package root on both sides.
PUB_CACHE="${PUB_CACHE:-$HOME/.pub-cache}"

# Dotted-version compare, patch-padded: exits 0 when $1 >= $2. Pre-release
# and build-metadata segments collapse to their numeric prefix — the
# fallback only needs coarse ordering between plain release versions.
ver_ge() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    na = split(a, A, "."); nb = split(b, B, ".")
    for (i = 1; i <= 3; i++) {
      x = (i <= na ? A[i] + 0 : 0); y = (i <= nb ? B[i] + 0 : 0)
      if (x != y) exit (x > y ? 0 : 1)
    }
    exit 0
  }'
}

# Upper bound of a caret constraint, pub's rule: ^X.Y.Z -> (X+1).0.0 when
# X > 0, ^0.Y.Z -> 0.(Y+1).0.
caret_hi() {
  local v="${1%%[-+]*}" maj min
  maj="${v%%.*}"
  min="${v#*.}"; min="${min%%.*}"
  case "$maj" in ''|*[!0-9]*) maj=0 ;; esac
  case "$min" in ''|*[!0-9]*) min=0 ;; esac
  if [ "$maj" -gt 0 ]; then echo "$((maj + 1)).0.0"; else echo "0.$((min + 1)).0"; fi
}

# Does candidate version $1 satisfy pub constraint $2? Precise forms this
# resolver cannot honor (<=) fail closed — the caller skips the dependency
# rather than pinning a wrong version.
satisfies() {
  local cand="$1" con="${2//\"/}" lo hi lstrict=0
  con="${con// /}"
  case "$con" in
    ""|"any") return 0 ;;
    "^"*)
      lo="${con#^}"; hi=$(caret_hi "$lo")
      ;;
    *"<="*) return 1 ;;
    *"<"*)
      lo="${con%%<*}"; hi="${con##*<}"
      case "$lo" in
        ">="*) lo="${lo#>=}"; lstrict=0 ;;
        ">"*)  lo="${lo#>}";  lstrict=1 ;;
        *)     lstrict=0 ;;
      esac
      hi="${hi#=}"
      ;;
    *">"*)
      lo="${con#*>}"; lo="${lo#=}"
      [ "${con:0:2}" = ">=" ] && lstrict=0 || lstrict=1
      hi=""
      ;;
    *)
      lo="$con"; hi="$con"
      ;;
  esac
  ver_ge "$cand" "$lo" || return 1
  if [ "$lstrict" = 1 ] && ver_ge "$lo" "$cand"; then return 1; fi
  if [ -n "$hi" ]; then
    if [ "$lo" = "$con" ]; then
      ver_ge "$hi" "$cand" || return 1
    elif ver_ge "$cand" "$hi"; then
      return 1
    fi
  fi
  return 0
}

# Highest cached version of $1 satisfying constraint $2; empty when the
# cache has no matching release.
pick_version() {
  local name="$1" con="$2" host="$PUB_CACHE/hosted/pub.dev"
  local d base ver best=""
  for d in "$host/$name-"*; do
    [ -d "$d" ] || continue
    base="${d##*/}"; ver="${base#"$name"-}"
    if ! satisfies "$ver" "$con"; then continue; fi
    if [ -z "$best" ]; then best="$ver"; continue; fi
    if ver_ge "$ver" "$best" && ! ver_ge "$best" "$ver"; then best="$ver"; fi
  done
  printf '%s' "$best"
}

# Hosted (name, exact version) pairs from a pubspec.lock, tab-separated.
lock_hosted_pairs() {
  local lock="$1"
  [ -f "$lock" ] || return 0
  awk '
    /^packages:/ { inpkg = 1; next }
    /^[^ #]/     { inpkg = 0; pkg = ""; src = ""; ver = "" }
    inpkg && /^  [^ ].*:$/ {
      if (src == "hosted" && ver != "") printf "%s\t%s\n", pkg, ver
      pkg = $1; sub(/:$/, "", pkg); src = ""; ver = ""
      next
    }
    pkg != "" && /^    source:/ { src = $2; next }
    pkg != "" && /^    version:/ { ver = $2; gsub(/"/, "", ver); next }
    END { if (inpkg && src == "hosted" && ver != "") printf "%s\t%s\n", pkg, ver }
  ' "$lock"
}

# Direct dependency lines from a pubspec.yaml as `name<TAB>constraint`;
# sdk:/path:/git:/hosted:-url deps yield `name<TAB>skip` (not resolvable
# from the hosted cache here).
yaml_direct_deps() {
  awk '
    function flush() { if (sec && name != "" && kind != "") print name "\t" kind }
    /^(dependencies|dev_dependencies):$/ { flush(); sec = 1; name = ""; kind = ""; next }
    /^[^ #]/ { flush(); sec = 0; name = ""; kind = ""; next }
    sec && /^  [A-Za-z0-9_]+:/ {
      flush()
      name = $0; sub(/^  /, "", name); sub(/:.*/, "", name)
      rest = $0; sub(/^  [A-Za-z0-9_]+:[ \t]*/, "", rest)
      kind = rest
      next
    }
    sec && kind == "" && /^    [a-z_]+:/ {
      k = $0; sub(/^ +/, "", k); sub(/:.*/, "", k)
      v = $0; sub(/^ +[a-z_]+:[ \t]*/, "", v)
      if (k == "version") kind = v
      else if (k == "sdk" || k == "path" || k == "git" || k == "hosted") kind = "skip"
      next
    }
    END { flush() }
  ' "$1"
}

# Language version pub derives for a cached package: major.minor of the
# lower bound of its pubspec.yaml sdk constraint; empty when undeclarable.
cached_langver() {
  local v
  v=$(awk '/^environment:/ { env = 1; next } /^[^ #]/ { env = 0 } env && /^  sdk:/ { sub(/^  sdk:[ \t]*/, ""); print; exit }' "$1/pubspec.yaml" 2>/dev/null || true)
  v="${v//\"/}"
  case "$v" in
    "^"*) v="${v#^}" ;;
    ">="*) v="${v#>=}" ;;
  esac
  printf '%s' "$v" | grep -oE '^[0-9]+\.[0-9]+' || true
}

write_fallback_config() {
  # Two statements: expanding $pkg in the same `local` list only works when
  # the caller happens to carry a same-named variable (set -u landmine).
  local pkg="$1"
  local dir="$CORPUS/$pkg"
  local host="$PUB_CACHE/hosted/pub.dev"
  local entries name con ver lang root pairs seen sib
  entries=" {\"name\": \"$pkg\", \"rootUri\": \"../\", \"packageUri\": \"lib/\", \"languageVersion\": \"3.13\"}"
  # Vendored zero-dep siblings: the corpus backbone, always preferred over
  # cache copies of the same packages.
  for sib in args:3.3 collection:3.4 meta:3.5 path:3.4; do
    [ -d "$CORPUS/${sib%%:*}" ] || continue
    entries+=",
 {\"name\": \"${sib%%:*}\", \"rootUri\": \"../../${sib%%:*}\", \"packageUri\": \"lib/\", \"languageVersion\": \"${sib##*:}\"}"
  done
  # Direct dependencies resolvable from the local pub cache.
  pairs=$(lock_hosted_pairs "$dir/pubspec.lock")
  seen=" "
  while IFS=$'\t' read -r name con; do
    [ -n "$name" ] || continue
    [ "$name" != "$pkg" ] || continue
    case "$seen" in *" $name "*) continue ;; esac
    seen="$seen$name "
    [ "$con" != "skip" ] || continue
    case " args collection meta path " in *" $name "*) continue ;; esac
    ver=$(printf '%s\n' "$pairs" | awk -F'\t' -v n="$name" '$1 == n { print $2; exit }')
    if [ -z "$ver" ]; then
      ver=$(pick_version "$name" "$con")
    fi
    [ -n "$ver" ] || continue
    [ -d "$host/$name-$ver" ] || continue
    lang=$(cached_langver "$host/$name-$ver")
    [ -n "$lang" ] || lang="3.0"
    root="file://$host/$name-$ver"
    entries+=",
 {\"name\": \"$name\", \"rootUri\": \"${root// /%20}\", \"packageUri\": \"lib/\", \"languageVersion\": \"$lang\"}"
  done <<< "$(yaml_direct_deps "$dir/pubspec.yaml")"
  mkdir -p "$dir/.dart_tool"
  printf '{"configVersion": 2, "generator": "pub", "packages": [\n%s\n]\n}\n' "$entries" >"$dir/.dart_tool/package_config.json"
  # Preserve a real lock (it carries the exact versions this config was
  # built from); the minimal lock is only a first-run seed.
  if [ ! -f "$dir/pubspec.lock" ]; then
    printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$dir/pubspec.lock"
  fi
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
# After each run, the per-side URI_DOES_NOT_EXIST counts are recorded to
# $URIMISS. Both engines must report the same count: a divergence means one
# side resolves imports the other cannot — a broken-resolution artifact
# (the #2352 pseudo-baseline shape), not a semantic finding — and fails
# the sweep.
echo "real-corpus-sweep: face 2 — differential (all severities)"
HARNESS_ERRORS=0
URIMISS_DIVERGENCES=0
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
    counts=$'-\t-'
  else
    echo "real-corpus-sweep: [$pkg] diff face done (exit $status; 1 = divergences persisted) — $OUT/$pkg.diff.log"
    counts=$'-\t-'
    if [ -f "$OUT/diff/$pkg/raw-outputs.json" ]; then
      counts=$(python3 - "$OUT/diff/$pkg/raw-outputs.json" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        raw = json.load(fh)
except Exception:
    print("-\t-")
    raise SystemExit(0)

def miss(side):
    return sum(
        1
        for corpus in raw.get(side, {}).get("corpora", [])
        for diag in corpus.get("diagnostics", [])
        if str(diag.get("code", "")).lower() == "uri_does_not_exist"
    )

print(f"{miss('dartq')}\t{miss('oracle')}")
PY
)
    fi
  fi
  printf '%s\t%s\n' "$pkg" "$counts" >>"$URIMISS"
  dq="${counts%%$'\t'*}"
  oc="${counts##*$'\t'}"
  if [ "$dq" != "-" ] && [ "$dq" != "$oc" ]; then
    URIMISS_DIVERGENCES=$((URIMISS_DIVERGENCES + 1))
    echo "real-corpus-sweep: [$pkg] URI_DOES_NOT_EXIST divergence — dartq=$dq oracle=$oc (broken-resolution artifact)" >&2
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

# --- fold per-side URI_DOES_NOT_EXIST counts into resolution.tsv ----------------
# Only the materialization rows carry the two extra columns; the diff/bench
# annotation rows keep their shape.
awk -F'\t' -v OFS='\t' '
  NR == FNR { if (FNR > 1) miss[$1] = $2 "\t" $3; next }
  FNR == 1 { print $0, "uri_miss_dq", "uri_miss_oracle"; next }
  ($2 == "kept" || $2 == "pub-offline" || $2 == "fallback" || $2 == "failed") && ($1 in miss) { print $0, miss[$1]; next }
  { print }
' "$URIMISS" "$RESOLUTION" >"$RESOLUTION.folded" && mv "$RESOLUTION.folded" "$RESOLUTION"

python3 "$AGG" "$OUT"

echo "real-corpus-sweep: complete — evidence in $OUT (summary.md, resolution.tsv, diff/, bench/, cases/)"
cat "$RESOLUTION"
if [ "$HARNESS_ERRORS" -ne 0 ]; then
  echo "real-corpus-sweep: FAILED faces present — see resolution.tsv notes and per-package logs" >&2
  exit 2
fi
if [ "$URIMISS_DIVERGENCES" -ne 0 ]; then
  echo "real-corpus-sweep: FAILED — $URIMISS_DIVERGENCES package(s) with divergent URI_DOES_NOT_EXIST counts (one engine is reading a different resolution tree; see uri_miss.tsv)" >&2
  exit 2
fi
exit 0
