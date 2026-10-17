#!/usr/bin/env bash
# mega-x-sweep.sh — generated mega-x corpus differential sweep (ticket #1812,
# axis 3d; the sweep-face counterpart of scripts/mega-x-run.sh's leg gate).
#
# Sweeps one generated mega-x tree (corpus-gen --scale <1k|10k|100k>
# --seed 0, optionally --layout workspace / --version-ladder) through the
# standard differential harness: oracle `dart analyze` vs `dartq analyze
# --format=machine`, all severities judged, divergences persisted in the
# standard regression-case-v1 format. No bespoke diff logic — the harness
# owns the (severity, code, file) caliber.
#
# Corpus-face scope: the generated tree is a REAL multi-package project —
# root `app` package plus `packages/core` + `packages/feature_*` path deps
# (flat layout), or a pub workspace of `resolution: workspace` members
# plus `third_party/` vendored non-members (workspace layout) — so the
# materialization face is a multi-entry package_config.json, not the
# single self entry the recovery/co19 sweeps write. The config mirrors
# scripts/mega-x-run.sh's materialize_config shape exactly: one entry per
# package found by scanning `pubspec.yaml` files, config-file-relative
# rootUri, `packageUri: "lib/"`, `languageVersion` = the lower bound of
# each pubspec's `environment.sdk` constraint (pub's settled-version rule).
#
# zadv/ adversarial overlay: rides along by default — storm files are
# diagnostics-by-design on BOTH sides (the oracle sees the same bytes), so
# agreed noise is a legitimate face. --skip-zadv clones the tree minus
# `zadv/` for the ruled-clean regime (mega-x-run.sh's X2 subset face made
# standalone).
#
# Materialization ruling (global; measured, not assumed):
#   The harness preflight requires one package root per run (pubspec.yaml +
#   .dart_tool/package_config.json with a self entry `rootUri: "../"` whose
#   languageVersion is >= 3.13). The generated tree already IS a package
#   root (app pubspec + member pubspecs); what it lacks is the resolution
#   artifact — the generated corpus deliberately carries no
#   `.dart_tool/package_config.json` (corpus-gen output is pubspec-level
#   only; the config is materialized by whoever runs it, the same ruling
#   mega-x-run.sh makes for its legs). Staging therefore clones the whole
#   tree and writes the config the tree's own pubspecs describe — the
#   corpus tree is NEVER written to. Clone is APFS clonefile with
#   plain-copy fallback (sweep-family convention); symlinks were rejected
#   (recursive walkers do not follow symlinked dirs).
#
# Version caliber (parametrized): the oracle SDK is whatever
#   DARTQ_DART_SDK points at (variable). The generated corpus pins
#   `^3.13.0` per pubspec by default; under --version-ladder the per-package
#   constraints span 2.12 -> 3.15 and the materialized languageVersion
#   follows each pubspec's own floor — the mixed-version face is real
#   coverage, read straight from the tree, not assumed.
#
# Triage caliber: divergences are clustered by (side, severity, code) with
#   entry/instance/file counts. NEVER per-file triage at sweep scale —
#   per-file attribution data lives in diff/report.json and
#   diff/raw-outputs.json; cases/ holds the persisted divergence cases.
#
# Divergences are discoveries, not sweep failures: the sweep exits 0 with
# them persisted and clustered. Exit 2 = preflight or harness/infra error.
#
# SERIAL PHASE ONLY: the real run executes oracle + dartq over up to ~100k
# files (memory-incident discipline). Never alongside development lanes:
#   DARTQ_MEGAX_CORPUS=tests/corpus/mega-x/1k \
#   DARTQ_DART_SDK=<pinned dart sdk root> \
#     scripts/mega-x-sweep.sh --label mega-x-sweep-r1
#
# Usage: scripts/mega-x-sweep.sh [--label L] [--dry-run] [--skip-zadv]
#   --label L    evidence dir name under target/evidence/mega-x-sweep/
#                (default MEGAX_SWEEP_LABEL env, else mega-x-sweep-r1). An
#                existing directory refuses to run — recorded evidence is
#                never overwritten (bump the label).
#   --dry-run    read-only corpus statistics plus the exact planned
#                commands; executes nothing. Requires DARTQ_MEGAX_CORPUS,
#                not DARTQ_DART_SDK.
#   --skip-zadv  sweep the ruled-clean regime: the staged tree drops the
#                zadv/ adversarial overlay (patho + unicode + errstorm),
#                the same face mega-x-run.sh materializes for leg X2.
# Env:   DARTQ_MEGAX_CORPUS  path to the generated mega-x tree (required,
#                          always — regenerate with corpus-gen first).
#        DARTQ_DART_SDK      pinned oracle Dart SDK root (>= 3.13);
#                          required unless --dry-run.
#        DARTQ_CMD           dartq side template; default
#                          "./target/release/dartq analyze
#                          --format=machine --sdk <DARTQ_DART_SDK>".
#        HARNESS_BIN         default ./target/release/harness.
set -euo pipefail
# Locale-stable byte order for the sort/uniq summary sites.
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

LABEL="${MEGAX_SWEEP_LABEL:-mega-x-sweep-r1}"
DRY_RUN=0
SKIP_ZADV=0
CORPUS="${DARTQ_MEGAX_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --skip-zadv) SKIP_ZADV=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "mega-x-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if [ -z "$CORPUS" ]; then
  echo "mega-x-sweep: DARTQ_MEGAX_CORPUS is required — set it to a generated mega-x tree," >&2
  echo "  e.g. DARTQ_MEGAX_CORPUS=tests/corpus/mega-x/1k scripts/mega-x-sweep.sh --dry-run" >&2
  echo "  (regenerate first: cargo run -p corpus-gen -- --out tests/corpus/mega-x/1k --scale 1k --seed 0)" >&2
  exit 2
fi
if [ ! -d "$CORPUS" ]; then
  echo "mega-x-sweep: DARTQ_MEGAX_CORPUS=$CORPUS is not a directory" >&2
  exit 2
fi
CORPUS="$(cd "$CORPUS" && pwd)"

# Corpus integrity probe: a generated mega-x tree carries manifest.json
# (corpus-gen's own fingerprint) plus the app pubspec; a tree without them
# is a mispointed path, not this corpus.
if [ ! -f "$CORPUS/manifest.json" ] || [ ! -f "$CORPUS/pubspec.yaml" ]; then
  echo "mega-x-sweep: $CORPUS does not look like a generated mega-x tree (no manifest.json + pubspec.yaml at the root)" >&2
  echo "  regenerate it: cargo run -p corpus-gen -- --out <dir> --scale <1k|10k|100k> --seed 0" >&2
  exit 2
fi

# The generated corpus pins per-package languageVersion by pubspec
# `environment.sdk` lower bound — read it back for the manifest and the
# package_config materialization (the version ladder's mixed face).
sdk_of() { # $1 = package dir -> prints its environment.sdk constraint
  sed -n 's/^  sdk: *//p' "$1/pubspec.yaml" | head -n1
}
lang_version_of() { # $1 = package dir -> major.minor lower bound
  sdk_of "$1" | sed -E 's/^\^?([0-9]+\.[0-9]+).*/\1/'
}

# --- corpus statistics (read-only; also recorded pre-run in real runs) ------
scan_stats() { # $1 = scan root — prints the report to stdout
  local scan="$1"
  local files top_dirs banners banner_versions pkgs storms patho uni
  files=$(find "$scan" -name '*.dart' -type f | wc -l | tr -d ' ')
  top_dirs=$(find "$scan" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
  banners=$(grep -rlE "^// ?@dart *=" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  banner_versions=$(grep -rhoE "^// ?@dart *= *[0-9]+\.[0-9]+" --include='*.dart' "$scan" 2>/dev/null \
    | grep -oE '[0-9]+\.[0-9]+$' | sort -u | tr '\n' ' ' || true)
  pkgs=$(find "$scan" -name pubspec.yaml -type f | wc -l | tr -d ' ')
  storms=$(find "$scan/zadv" -name 'errstorm_*' -type d 2>/dev/null | wc -l | tr -d ' ')
  patho=$(find "$scan/zadv/patho" -name '*.dart' 2>/dev/null | wc -l | tr -d ' ')
  uni=$(find "$scan/zadv/unicode" -name '*.dart' 2>/dev/null | wc -l | tr -d ' ')

  echo "corpus: $scan"
  echo "  files: $files  top-level dirs: $top_dirs  pubspec packages: $pkgs"
  echo "  @dart banner files: $banners  versions: ${banner_versions:-none}"
  if [ "$SKIP_ZADV" -eq 1 ]; then
    echo "  zadv overlay: $storms storm packages, $patho patho files, $uni unicode files (--skip-zadv drops the whole overlay)"
  else
    echo "  zadv overlay: $storms storm packages, $patho patho files, $uni unicode files (rides along — agreed noise on both sides)"
  fi
  echo "  sdk constraint spread (per pubspec):"
  find "$scan" -name pubspec.yaml -type f | sort | while read -r spec; do
    d="$(dirname "$spec")"
    printf '    %-56s %s\n' "${d#"$scan"/}" "$(sdk_of "$d")"
  done | head -40
}

echo "mega-x-sweep: label=$LABEL corpus=$CORPUS$( [ "$SKIP_ZADV" -eq 1 ] && echo " (zadv/ dropped)" )"
scan_stats "$CORPUS"

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $sdk_desc}"
  echo "mega-x-sweep: DRY RUN — statistics above; real run would execute:"
  echo "  materialize: cp -Rc $CORPUS -> target/evidence/mega-x-sweep/$LABEL/staging (clonefile, cp fallback)" \
    "$( [ "$SKIP_ZADV" -eq 1 ] && echo '; rm -rf staging/zadv ' )" \
    "; write staging/.dart_tool/package_config.json (one entry per pubspec, per-pubspec languageVersion)"
  echo "  diff: ${HARNESS_BIN:-./target/release/harness} run --corpus-dir target/evidence/mega-x-sweep/$LABEL/staging \\"
  echo "    --oracle-dart $sdk_desc/bin/dart --dartq-cmd \"$dartq_cmd\" --severity-filter all --allow-vacuous \\"
  echo "    --persist-regressions target/evidence/mega-x-sweep/$LABEL/cases --out target/evidence/mega-x-sweep/$LABEL/diff"
  echo "  triage: fold diff/report.json into triage.{json,md} clustered by (side, severity, code)"
  exit 0
fi

# --- real run preflight -------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "mega-x-sweep: DARTQ_DART_SDK is required for a real run (pinned oracle Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "mega-x-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "mega-x-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "mega-x-sweep: harness/dartq binaries missing after build" >&2
  exit 2
fi
DARTQ_CMD="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $DARTQ_DART_SDK}"

OUT="target/evidence/mega-x-sweep/$LABEL"
if [ -d "$OUT" ]; then
  echo "mega-x-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"

# --- face 1: manifest ----------------------------------------------------------
CORPUS_VERSION="generated corpus (corpus-gen, seed 0)"
if commit=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null); then
  CORPUS_VERSION="generated corpus (corpus-gen, seed 0, commit $commit)"
fi
MANIFEST="$OUT/versions.tsv"
{
  printf 'key\tvalue\n'
  printf 'oracle_sdk\t%s\n' "$DARTQ_DART_SDK"
  printf 'corpus_tree\t%s\n' "$CORPUS"
  printf 'corpus_tree_version\t%s\n' "$CORPUS_VERSION"
  printf 'corpus_language_version\tper-pubspec (materialized config follows each environment.sdk floor)\n'
  printf 'zadv_overlay\t%s\n' "$( [ "$SKIP_ZADV" -eq 1 ] && echo dropped || echo riding )"
  printf 'dartq_cmd\t%s\n' "$DARTQ_CMD"
} >"$MANIFEST"
scan_stats "$CORPUS" >"$OUT/stats.txt"
cat "$MANIFEST"

# --- face 2: materialization ----------------------------------------------------
clone_dir() { # $1 = src dir, $2 = dst dir — clonefile with plain-copy fallback
  if cp -Rc "$1/." "$2/" 2>/dev/null; then
    echo clone
  else
    cp -R "$1/." "$2/"
    echo copy
  fi
}
STAGE_ROOT="$OUT/staging"
mkdir -p "$STAGE_ROOT"
MATERIALIZE_MODE="corpus:$(clone_dir "$CORPUS" "$STAGE_ROOT")"
if [ "$SKIP_ZADV" -eq 1 ]; then
  rm -rf "$STAGE_ROOT/zadv"
  MATERIALIZE_MODE="$MATERIALIZE_MODE zadv:dropped"
fi
printf 'materialization\t%s\n' "$MATERIALIZE_MODE" >>"$MANIFEST"

# package_config.json: one entry per pubspec found in the staged tree —
# rootUri relative to the config file (.dart_tool/ is one level deep, so a
# package at <rel> gets rootUri ../<rel>; the root package gets ../).
# languageVersion = the pubspec's environment.sdk lower bound, matching
# what pub derives (the nested-versions corpus documents the rule).
mkdir -p "$STAGE_ROOT/.dart_tool"
config="$STAGE_ROOT/.dart_tool/package_config.json"
entries='[]'
while IFS= read -r spec; do
  d="$(dirname "$spec")"
  rel="${d#"$STAGE_ROOT"}"
  rel="${rel#/}"
  name="$(sed -n 's/^name: *//p' "$spec" | head -n1)"
  [ -n "$name" ] || continue
  ver="$(lang_version_of "$d")"
  if [ -z "$rel" ]; then
    uri=".."
  else
    uri="../$rel"
  fi
  entries="$(printf '%s' "$entries" | jq -c \
    --arg n "$name" --arg r "$uri" --arg v "$ver" \
    '. + [{name: $n, rootUri: $r, packageUri: "lib/", languageVersion: $v}]')" \
    || { echo "mega-x-sweep: package_config assembly failed for $d" >&2; exit 2; }
done < <(find "$STAGE_ROOT" -name pubspec.yaml -type f | sort)
jq -n --argjson p "$entries" '{configVersion: 2, generator: "pub", packages: $p}' >"$config"

# --- face 3: differential -------------------------------------------------------
# harness exit codes: 0 pass, 1 divergences found (discovery — cases
# persisted), 2 harness/infra error (fatal for the sweep).
echo "mega-x-sweep: face 3 — differential (all severities)"
status=0
"$HARNESS_BIN" run \
  --corpus-dir "$STAGE_ROOT" \
  --oracle-dart "$DARTQ_DART_SDK/bin/dart" \
  --dartq-cmd "$DARTQ_CMD" \
  --severity-filter all \
  --allow-vacuous \
  --persist-regressions "$OUT/cases" \
  --out "$OUT/diff" >"$OUT/harness.log" 2>&1 || status=$?
if [ "$status" -eq 2 ]; then
  echo "mega-x-sweep: harness error (exit 2) — see $OUT/harness.log" >&2
  exit 2
fi
echo "mega-x-sweep: diff done (exit $status; 1 = divergences persisted) — $OUT/harness.log"

# --- face 4: triage (code-clustered, never per-file) ----------------------------
python3 - "$OUT" "$CORPUS" <<'PY'
import collections
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
face = sys.argv[2]
rep = json.loads((out / "diff" / "report.json").read_text(encoding="utf-8"))

manifest = {}
tsv = out / "versions.tsv"
if tsv.exists():
    for line in tsv.read_text(encoding="utf-8").splitlines()[1:]:
        fields = line.split("\t", 1)
        if len(fields) == 2:
            manifest[fields[0]] = fields[1]

clusters = []
for side, rows in (("oracle-only", rep.get("oracle_only") or []),
                   ("dartq-only", rep.get("dartq_only") or [])):
    grouped = collections.defaultdict(lambda: {"entries": 0, "instances": 0, "files": set()})
    for row in rows:
        key = (row.get("severity", ""), row.get("code", ""))
        g = grouped[key]
        g["entries"] += 1
        g["instances"] += row.get("count", 0)
        g["files"].add(row.get("file", ""))
    for (severity, code), g in sorted(grouped.items()):
        clusters.append({"side": side, "severity": severity, "code": code,
                         "entries": g["entries"], "instances": g["instances"],
                         "files": len(g["files"])})
clusters.sort(key=lambda c: -c["instances"])

summary = {
    "schema": "mega-x-sweep-triage-v1",
    "face": face,
    "oracle_version": (rep.get("meta") or {}).get("oracle_version", ""),
    "corpus_tree_version": manifest.get("corpus_tree_version", "unknown"),
    "zadv_overlay": manifest.get("zadv_overlay", "riding"),
    "passed": rep.get("passed"),
    "error_jaccard": rep.get("error_jaccard"),
    "file_count": rep.get("file_count"),
    "oracle_total": rep.get("oracle_total"),
    "dartq_total": rep.get("dartq_total"),
    "severities": [{"severity": s["severity"], "jaccard": s["jaccard"]}
                   for s in rep.get("severities", [])],
    "clusters": clusters,
}
(out / "triage.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")

md = [f"# Mega-x sweep triage — {out.name}", "",
      f"face: {face} | oracle: {summary['oracle_version']} | "
      f"corpus tree: {summary['corpus_tree_version']} | "
      f"zadv: {summary['zadv_overlay']} | files: {summary['file_count']}", "",
      f"files with diagnostics judged; oracle {summary['oracle_total']} diags, "
      f"dartq {summary['dartq_total']} diags; ERROR jaccard {summary['error_jaccard']}, "
      f"passed: {summary['passed']}", "",
      "Per-severity jaccard: "
      + ", ".join(f"{s['severity']}={s['jaccard']:.3f}" for s in summary["severities"]), "",
      "Diff entry clusters (side, severity, code):", "",
      "| side | severity | code | entries | instances | files |",
      "| --- | --- | --- | --- | --- | --- |"]
for c in clusters:
    md.append("| {side} | {severity} | {code} | {entries} | {instances} | {files} |".format(**c))
md += ["", "Caliber: clustered counts only — per-file attribution lives in "
       "diff/report.json and diff/raw-outputs.json; cases/ holds the persisted "
       "divergence cases (regression-case-v1). Generated corpus: fixture bytes "
       "derive from corpus-gen seed 0; pair with the oracle version when "
       "attributing version-shaped clusters."]
(out / "triage.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print(f"triage: {len(clusters)} diff cluster(s) -> {out / 'triage.md'}")
PY

echo "mega-x-sweep: complete — evidence in $OUT (stats.txt, versions.tsv, diff/, cases/, triage.md)"
[ "$status" -eq 1 ] && echo "mega-x-sweep: divergences found and persisted — discoveries, attribute via triage.md clusters"
exit 0
