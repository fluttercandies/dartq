#!/usr/bin/env bash
# language-sweep.sh — official SDK tests/language differential sweep (ticket #1535).
#
# Sweeps the official `tests/language` tree (4,559 files, 130+ feature dirs)
# through the standard differential harness: oracle `dart analyze` vs
# `dartq analyze --format=json`, all severities judged, divergences
# persisted in the standard regression-case-v1 format. No bespoke diff
# logic — the harness owns the (severity, code, file) caliber.
#
# Materialization ruling (global, not per-feature):
#   The harness preflight requires one package root per run (pubspec.yaml +
#   .dart_tool/package_config.json with a self entry `rootUri: "../"` whose
#   languageVersion is >= 3.13). tests/language is materialized as ONE
#   package because:
#     1. ~160 relative imports (`../`) cross feature dirs; per-feature
#        staging would push them outside the analyzed package root — a
#        resolution artifact, not semantics. Global staging preserves the
#        original path depth (`staging/tests/language/...`), so every
#        relative URI resolves exactly as in the source tree.
#     2. One run = one oracle process + one dartq process over the whole
#        tree; per-feature materialization means ~180 process pairs.
#     3. Named `part of` resolution sees the full library set on both
#        sides — cross-dir library-name ambiguity is a shared semantic
#        face (a legitimate differential finding), not one-sided noise.
#   The corpus tree is NEVER written to: staging is a clone (APFS
#   clonefile, plain-copy fallback) under target/evidence/. Symlinked
#   subtrees were rejected: recursive directory walkers do not follow
#   symlinked dirs, which would silently scan zero files.
#
# package: resolution: every `package:<name>/` referenced by the corpus is
#   mapped to `<sdk-root>/pkg/<name>` when that package exists (expect,
#   async_helper, meta, dart_internal, js — discovered dynamically, not
#   hardcoded). All other targets (none in the official helper set today)
#   stay unmapped and are quantified in the stats output — both sides see
#   the identical unresolved set, so it is agreed noise, not differential
#   noise. Package languageVersion is pinned 3.13 (the harness Plan A
#   floor) for the self package and all mapped helpers; files carrying an
#   explicit `// @dart = x.y` banner override per file — the
#   mixed-versions face is real coverage, counted in the stats.
#
# Version caliber (parametrized): the oracle SDK is whatever DARTQ_DART_SDK
#   points at (variable). The corpus tree's own version (e.g. a 3.15 main
#   checkout analyzed by a 3.13 oracle) is read from `<sdk>/tools/VERSION`
#   when present and recorded in versions.tsv; the run-time oracle version
#   captured by the harness (report meta.oracle_version) is the attribution
#   truth folded into triage.md. Version-drift divergences are discoveries
#   to be attributed, not sweep failures.
#
# Triage caliber: divergences are clustered by (side, severity, code) with
#   entry/instance/file counts. NEVER per-file triage at sweep scale —
#   per-file attribution data lives in diff/report.json and
#   diff/raw-outputs.json; cases/ holds the persisted divergence cases
#   (known-diffs exemption table still applies).
#
# Divergences are discoveries, not sweep failures: the sweep exits 0 with
# them persisted and clustered. Exit 2 = preflight or harness/infra error.
#
# SERIAL PHASE ONLY: the real run executes oracle + dartq over 4.5k files
# (memory-incident discipline). Never alongside development lanes:
#   DARTQ_LANGUAGE_CORPUS=<sdk>/tests/language \
#   DARTQ_DART_SDK=<pinned dart sdk root> \
#     scripts/language-sweep.sh --label lang-r1
#
# Usage: scripts/language-sweep.sh [--label L] [--dry-run] [--only <dir>]
#   --label L    evidence dir name under target/evidence/language-sweep/
#                (default LANGUAGE_SWEEP_LABEL env, else lang-r1). An
#                existing directory refuses to run — recorded evidence is
#                never overwritten (bump the label).
#   --dry-run    read-only noise/scale statistics over the corpus (file
#                counts, package: import surface mapped/unmapped, part-of
#                styles, @dart banners, cross-dir imports) plus the exact
#                planned commands; executes nothing. Requires
#                DARTQ_LANGUAGE_CORPUS, not DARTQ_DART_SDK.
#   --only <dir> restrict the sweep to one top-level feature dir under
#                tests/language (e.g. --only async). Relative imports
#                escaping the selected dir then resolve to nothing on both
#                sides equally.
# Env:   DARTQ_LANGUAGE_CORPUS  path to the official tests/language tree
#                                (required, always — the stats read it).
#        DARTQ_DART_SDK         pinned oracle Dart SDK root (>= 3.13);
#                                required unless --dry-run.
#        DARTQ_CMD              dartq side template; default
#                               "./target/release/dartq analyze
#                               --format=json --sdk <DARTQ_DART_SDK>".
#        HARNESS_BIN            default ./target/release/harness.
set -euo pipefail
# Locale-stable byte order for the sort/uniq summary sites (precedent: scripts/incremental-gate.sh).
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

LABEL="${LANGUAGE_SWEEP_LABEL:-lang-r1}"
DRY_RUN=0
ONLY=""
CORPUS="${DARTQ_LANGUAGE_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --only) ONLY="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "language-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if [ -z "$CORPUS" ]; then
  echo "language-sweep: DARTQ_LANGUAGE_CORPUS is required — set it to the official tests/language tree," >&2
  echo "  e.g. DARTQ_LANGUAGE_CORPUS=<dart-sdk-checkout>/tests/language scripts/language-sweep.sh --dry-run" >&2
  exit 2
fi
if [ ! -d "$CORPUS" ]; then
  echo "language-sweep: DARTQ_LANGUAGE_CORPUS=$CORPUS is not a directory" >&2
  exit 2
fi
CORPUS="$(cd "$CORPUS" && pwd)"
SDK_ROOT="$(cd "$CORPUS/../.." && pwd)"

SCAN="$CORPUS"
SCAN_DESC="tests/language"
if [ -n "$ONLY" ]; then
  if [ ! -d "$CORPUS/$ONLY" ]; then
    echo "language-sweep: --only '$ONLY' is not a directory under $CORPUS" >&2
    exit 2
  fi
  SCAN="$CORPUS/$ONLY"
  SCAN_DESC="tests/language/$ONLY"
fi

# --- corpus statistics (read-only; also recorded pre-run in real runs) ------
# The noise/scale surface this prints is the evidence behind the global
# materialization ruling above: measure it before trusting any run.
scan_stats() { # $1 = scan root — prints the report to stdout
  local scan="$1"
  local files top_dirs part_files named_po uri_po banners banner_versions xdir refs
  files=$(find "$scan" -name '*.dart' -type f | wc -l | tr -d ' ')
  top_dirs=$(find "$scan" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
  part_files=$(grep -rlE "^part ['\"]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  named_po=$(grep -rlE "^part of [A-Za-z_]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  uri_po=$(grep -rlE "^part of ['\"]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  banners=$(grep -rlE "^// ?@dart *=" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  banner_versions=$(grep -rhoE "^// ?@dart *= *[0-9]+\.[0-9]+" --include='*.dart' "$scan" 2>/dev/null \
    | grep -oE '[0-9]+\.[0-9]+$' | sort -u | tr '\n' ' ' || true)
  xdir=$(grep -rhoE "(import|export|part) +['\"](\.\./)+" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)

  echo "corpus: $scan"
  echo "  files: $files  top-level feature dirs: $top_dirs"
  echo "  part directive files: $part_files  (part-of named: $named_po, part-of uri: $uri_po)"
  echo "  @dart banner files: $banners  versions: ${banner_versions:-none}"
  echo "  cross-dir relative refs (../): $xdir  <- global materialization keeps these in-package"
  echo "  package: uri references (import/export) per target package:"
  refs=$(grep -rhoE "(import|export) +['\"]package:[A-Za-z_0-9]+/" --include='*.dart' "$scan" 2>/dev/null \
    | sed -E 's/.*package:([A-Za-z_0-9]+)\/.*/\1/' | sort | uniq -c | sort -rn || true)
  if [ -z "$refs" ]; then
    echo "    (none)"
  else
    printf '%s\n' "$refs" | while read -r count name; do
      if [ -d "$SDK_ROOT/pkg/$name" ]; then
        echo "    $count  package:$name  (mapped -> <sdk>/pkg/$name)"
      else
        echo "    $count  package:$name  (UNMAPPED — expected unresolved on both sides)"
      fi
    done
  fi
}

if [ ! -d "$SDK_ROOT/pkg/expect" ]; then
  echo "language-sweep: helper package not found at $SDK_ROOT/pkg/expect" >&2
  echo "  DARTQ_LANGUAGE_CORPUS must point at <dart-sdk-checkout>/tests/language so" >&2
  echo "  <dart-sdk-checkout>/pkg/expect (and siblings) are reachable at ../.." >&2
  exit 2
fi

echo "language-sweep: label=$LABEL corpus=$SCAN_DESC sdk-root=$SDK_ROOT"
scan_stats "$SCAN"

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=json --sdk $sdk_desc}"
  echo "language-sweep: DRY RUN — statistics above; real run would execute:"
  echo "  materialize: cp -Rc $SCAN -> target/evidence/language-sweep/$LABEL/staging/$SCAN_DESC (clonefile, cp fallback)" \
    "; write staging/{pubspec.yaml,pubspec.lock,.dart_tool/package_config.json} (self rootUri ../ languageVersion 3.13 + mapped pkg/ helpers)"
  echo "  diff: ${HARNESS_BIN:-./target/release/harness} run --corpus-dir target/evidence/language-sweep/$LABEL/staging \\"
  echo "    --oracle-dart $sdk_desc/bin/dart --dartq-cmd \"$dartq_cmd\" --severity-filter all --allow-vacuous \\"
  echo "    --persist-regressions target/evidence/language-sweep/$LABEL/cases --out target/evidence/language-sweep/$LABEL/diff"
  echo "  triage: fold diff/report.json into triage.{json,md} clustered by (side, severity, code)"
  exit 0
fi

# --- real run preflight -------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "language-sweep: DARTQ_DART_SDK is required for a real run (pinned oracle Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "language-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "language-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "language-sweep: harness/dartq binaries missing after build" >&2
  exit 2
fi
DARTQ_CMD="${DARTQ_CMD:-./target/release/dartq analyze --format=json --sdk $DARTQ_DART_SDK}"

OUT="target/evidence/language-sweep/$LABEL"
if [ -d "$OUT" ]; then
  echo "language-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"

# --- face 1: manifest ----------------------------------------------------------
# Version caliber on record before anything runs (see header).
CORPUS_VERSION="unknown"
if [ -f "$SDK_ROOT/tools/VERSION" ]; then
  CORPUS_VERSION=$(awk '$1=="CHANNEL"{c=$2} $1=="MAJOR"{ma=$2} $1=="MINOR"{mi=$2} $1=="PATCH"{p=$2} END{printf "%s %s.%s.%s", c, ma, mi, p}' "$SDK_ROOT/tools/VERSION")
fi
MANIFEST="$OUT/versions.tsv"
{
  printf 'key\tvalue\n'
  printf 'oracle_sdk\t%s\n' "$DARTQ_DART_SDK"
  printf 'corpus_tree\t%s\n' "$SCAN"
  printf 'corpus_tree_version\t%s\n' "$CORPUS_VERSION"
  printf 'corpus_language_version\t3.13 (package_config pin; @dart banner files override per file)\n'
  printf 'dartq_cmd\t%s\n' "$DARTQ_CMD"
} >"$MANIFEST"
scan_stats "$SCAN" >"$OUT/stats.txt"
cat "$MANIFEST"

# --- face 2: materialization ----------------------------------------------------
# Clone with the original path depth preserved (staging/tests/language/...)
# so relative URIs resolve exactly as in the source tree (see header).
STAGE_ROOT="$OUT/staging"
STAGE_CORPUS="$STAGE_ROOT/$SCAN_DESC"
mkdir -p "$STAGE_CORPUS"
if cp -Rc "$SCAN/." "$STAGE_CORPUS/" 2>/dev/null; then
  MATERIALIZE_MODE=clone
else
  cp -R "$SCAN/." "$STAGE_CORPUS/"
  MATERIALIZE_MODE=copy
fi
printf 'materialization\t%s\n' "$MATERIALIZE_MODE" >>"$MANIFEST"

printf 'name: language_corpus\nenvironment:\n  sdk: ^3.13.0\n' >"$STAGE_ROOT/pubspec.yaml"
printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$STAGE_ROOT/pubspec.lock"

# package_config.json: self entry exactly `rootUri: "../"` (the harness
# languageVersion preflight matches on it) + one file:// entry per observed
# package: reference whose <sdk>/pkg/<name> exists.
python3 - "$STAGE_ROOT" "$SDK_ROOT" "$SCAN" <<'PY'
import json
import pathlib
import re
import sys

stage, sdk_root, scan = (pathlib.Path(p) for p in sys.argv[1:4])
pat = re.compile(r"""(?:import|export)\s+['"]package:([A-Za-z_0-9]+)/""")
names = set()
for f in scan.rglob("*.dart"):
    try:
        text = f.read_text(encoding="utf-8", errors="replace")
    except OSError:
        continue
    names.update(pat.findall(text))
packages = [{"name": "language_corpus", "rootUri": "../", "languageVersion": "3.13"}]
for name in sorted(names):
    pkg = sdk_root / "pkg" / name
    if pkg.is_dir():
        packages.append({"name": name, "rootUri": pkg.resolve().as_uri(), "languageVersion": "3.13"})
config = {"configVersion": 2, "generator": "pub", "packages": packages}
(stage / ".dart_tool").mkdir(exist_ok=True)
(stage / ".dart_tool" / "package_config.json").write_text(
    json.dumps(config, indent=1) + "\n", encoding="utf-8")
print(f"package_config: {len(packages) - 1} mapped helper package(s): "
      + ", ".join(p["name"] for p in packages[1:]))
PY

# --- face 3: differential -------------------------------------------------------
# harness exit codes: 0 pass, 1 divergences found (discovery — cases
# persisted), 2 harness/infra error (fatal for the sweep).
echo "language-sweep: face 3 — differential (all severities)"
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
  echo "language-sweep: harness error (exit 2) — see $OUT/harness.log" >&2
  exit 2
fi
echo "language-sweep: diff done (exit $status; 1 = divergences persisted) — $OUT/harness.log"

# --- face 4: triage (code-clustered, never per-file) ----------------------------
python3 - "$OUT" "tests/language" <<'PY'
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
    "schema": "language-sweep-triage-v1",
    "face": face,
    "oracle_version": (rep.get("meta") or {}).get("oracle_version", ""),
    "corpus_tree_version": manifest.get("corpus_tree_version", "unknown"),
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

md = [f"# Language sweep triage — {out.name}", "",
      f"face: {face} | oracle: {summary['oracle_version']} | "
      f"corpus tree: {summary['corpus_tree_version']} | "
      f"language pin: 3.13 | files: {summary['file_count']}", "",
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
       "divergence cases (regression-case-v1)."]
(out / "triage.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print(f"triage: {len(clusters)} diff cluster(s) -> {out / 'triage.md'}")
PY

echo "language-sweep: complete — evidence in $OUT (stats.txt, versions.tsv, diff/, cases/, triage.md)"
[ "$status" -eq 1 ] && echo "language-sweep: divergences found and persisted — discoveries, attribute via triage.md clusters"
exit 0
