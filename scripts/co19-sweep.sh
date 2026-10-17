#!/usr/bin/env bash
# co19-sweep.sh — official co19 differential sweep over the Language,
# LanguageFeatures, LibTest and TypeSystem trees (tickets #1541, #1812).
#
# Sweeps one official co19 tree (Language 5,717 .dart files across 15
# feature dirs; LanguageFeatures ~6.7k; LibTest ~8.5k; TypeSystem ~3.9k)
# through the standard differential harness: oracle `dart analyze` vs
# `dartq analyze --format=machine`, all severities judged, divergences
# persisted in the standard regression-case-v1 format. No bespoke diff
# logic — the harness owns the (severity, code, file) caliber.
#
# Corpus-face scope: the tree DARTQ_CO19_CORPUS names — `Language`,
# `LanguageFeatures`, `LibTest` or `TypeSystem` (recognized by basename).
# Language itself has zero package: references; the other three trees do —
# files import `package:` targets resolved through the SDK pkg/ mapping
# face below (the language-sweep.sh mechanism transplanted): each
# referenced `package:<name>` that exists at `<sdk-checkout>/pkg/<name>`
# gets a package_config entry; unmapped targets stay unresolved
# identically on both sides (agreed noise, quantified in stats).
#
# Embedded expect annotations: co19 files carry `// expect`-style comment
# markers describing the official test runner's expectations. At analysis
# time they are plain comments — both engines see the identical bytes;
# nothing is stripped or interpreted by this script. Marker semantics
# belong to the official runner, not to the analyzer differential.
#
# .status files: none exist anywhere in the co19 repo (measured 0; the
# official skip/compile status metadata lives in the SDK checkout's test
# matrix, not in the co19 corpus). The runner skip set is therefore out of
# scope by construction; both engines analyze every .dart file found.
#
# Materialization ruling (global, not per-feature; measured, not assumed):
#   The harness preflight requires one package root per run (pubspec.yaml +
#   .dart_tool/package_config.json with a self entry `rootUri: "../"` whose
#   languageVersion is >= 3.13). Every swept tree is materialized as ONE
#   package because:
#     1. Language uses relative URIs exclusively (zero `package:` imports,
#        measured) and 1,733 of them escape their directory: 1,668 reach
#        the repo-root `Utils/` helper package (`Utils/expect.dart` x1,607,
#        plus static_type_helper / dynamic_check / test_mode_check), ~65
#        reach sibling test libraries (shared `lib.dart` fixtures). Escapes
#        run up to four levels deep (`../../../../Utils/expect.dart`) —
#        exactly the repo-root level. Staging therefore reproduces that
#        level: `staging/co19/<Language[/only]>` plus `staging/co19/Utils`,
#        so every relative URI resolves exactly as in the source tree.
#        Per-feature staging would strand the helper imports outside the
#        analyzed root — a resolution artifact, not semantics.
#        For the LanguageFeatures/LibTest/TypeSystem trees the helper face
#        differs: those trees import `package:` targets instead, resolved
#        through SDK pkg/ package_config entries (below) rather than the
#        repo-local Utils/ clone — Utils is staged when it exists, skipped
#        when the tree does not reference it.
#     2. One run = one oracle process + one dartq process over the whole
#        tree; per-feature materialization means ~180 process pairs.
#     3. Named `part of` resolution sees the full library set on both
#        sides — cross-dir library-name ambiguity is a shared semantic
#        face (a legitimate differential finding), not one-sided noise.
#   Unlike language-sweep.sh, the Language helper package is staged INSIDE
#   the analyzed package (co19's helpers are repo-local, not SDK pkg/
#   trees): ~20 Utils .dart files are themselves analyzed on both sides —
#   an agreed extra face, identical input both engines.
#   The corpus tree is NEVER written to: staging is a clone (APFS
#   clonefile, plain-copy fallback) under target/evidence/. Symlinked
#   subtrees were rejected: recursive directory walkers do not follow
#   symlinked dirs, which would silently scan zero files.
#
# package: resolution: none needed for Language (zero references,
# measured). For the LanguageFeatures/LibTest/TypeSystem trees, each
# referenced `package:<name>` that exists as `<sdk-checkout>/pkg/<name>`
# (a dart-lang/sdk source checkout, e.g. pkg/expect, pkg/meta) is mapped
# into the staged package_config.json — the language-sweep.sh face;
# anything else stays unmapped and is quantified in stats.txt — agreed
# noise, never differential noise. The mapping face needs a source
# checkout (compiled SDKs carry no pkg/ trees): DARTQ_SDK_CHECKOUT for
# these trees is REQUIRED and dies with an explicit error when unset —
# never a repo-local or absolute path baked into the script. Package
# languageVersion is pinned 3.13 (the harness Plan A floor); files
# carrying an explicit `// @dart = x.y` banner (29 in Language, more in
# the feature trees — measured per run) override per file — the
# mixed-versions face is real coverage, counted in the stats.
#
# Version caliber (parametrized): the oracle SDK is whatever DARTQ_DART_SDK
#   points at (variable). The co19 repo has no tools/VERSION — it is a
#   rolling-main corpus. versions.tsv records an explicit "rolling main"
#   declaration plus the best-effort git commit of the checkout (when git
#   metadata is present); the run-time oracle version captured by the
#   harness (report meta.oracle_version) is the attribution truth folded
#   into triage.md. Version-shaped divergences are attributed in triage by
#   pairing these two versions; they are discoveries, not sweep failures.
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
# SERIAL PHASE ONLY: the real run executes oracle + dartq over 5.7k files
# (memory-incident discipline). Never alongside development lanes:
#   DARTQ_CO19_CORPUS=<co19-checkout>/Language \
#   DARTQ_DART_SDK=<pinned dart sdk root> \
#     scripts/co19-sweep.sh --label co19-r1
#
# Usage: scripts/co19-sweep.sh [--label L] [--dry-run] [--only <dir>]
#   --label L    evidence dir name under target/evidence/co19-sweep/
#                (default CO19_SWEEP_LABEL env, else co19-r1). An existing
#                directory refuses to run — recorded evidence is never
#                overwritten (bump the label).
#   --dry-run    read-only noise/scale statistics over the corpus (file
#                counts, helper-import surface, cross-dir refs, part-of
#                styles, @dart banners) plus the exact planned commands;
#                executes nothing. Requires DARTQ_CO19_CORPUS, not
#                DARTQ_DART_SDK.
#   --only <dir> restrict the sweep to one top-level feature dir under the
#                corpus tree (e.g. --only Classes). The helper face is
#                staged as usual, so imports keep resolving; relative
#                imports escaping the selected dir otherwise resolve to
#                nothing on both sides equally.
# Env:   DARTQ_CO19_CORPUS  path to a co19 tree — Language,
#                          LanguageFeatures, LibTest or TypeSystem
#                          (required, always — the stats read it).
#        DARTQ_DART_SDK     pinned oracle Dart SDK root (>= 3.13);
#                           required unless --dry-run.
#        DARTQ_SDK_CHECKOUT dart-lang/sdk SOURCE checkout root (required
#                           when the corpus tree references package:
#                           targets — LanguageFeatures/LibTest/TypeSystem;
#                           the pkg/ helper-mapping source). Unset or
#                           missing -> explicit exit-2 error, never a
#                           repo-local fallback.
#        DARTQ_CMD          dartq side template; default
#                           "./target/release/dartq analyze
#                           --format=machine --sdk <DARTQ_DART_SDK>".
#        HARNESS_BIN        default ./target/release/harness.
set -euo pipefail
# Locale-stable byte order for the sort/uniq summary sites (precedent: scripts/incremental-gate.sh).
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

LABEL="${CO19_SWEEP_LABEL:-co19-r1}"
DRY_RUN=0
ONLY=""
CORPUS="${DARTQ_CO19_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --only) ONLY="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "co19-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if [ -z "$CORPUS" ]; then
  echo "co19-sweep: DARTQ_CO19_CORPUS is required — set it to the co19 repo's Language tree," >&2
  echo "  e.g. DARTQ_CO19_CORPUS=<co19-checkout>/Language scripts/co19-sweep.sh --dry-run" >&2
  exit 2
fi
if [ ! -d "$CORPUS" ]; then
  echo "co19-sweep: DARTQ_CO19_CORPUS=$CORPUS is not a directory" >&2
  exit 2
fi
CORPUS="$(cd "$CORPUS" && pwd)"
CO19_ROOT="$(cd "$CORPUS/.." && pwd)"

# Tree kind by basename: Language (repo-local Utils helpers, zero package:
# refs) or the pkg/-mapping trees (LanguageFeatures/LibTest/TypeSystem).
TREE_KIND="$(basename "$CORPUS")"
case "$TREE_KIND" in
  Language | LanguageFeatures | LibTest | TypeSystem) ;;
  *)
    echo "co19-sweep: unrecognized co19 tree '$TREE_KIND' — DARTQ_CO19_CORPUS must point at" >&2
    echo "  <co19-checkout>/{Language,LanguageFeatures,LibTest,TypeSystem}" >&2
    exit 2
    ;;
esac

# The pkg/ mapping face needs a dart-lang/sdk SOURCE checkout (compiled
# SDKs carry no pkg/ trees). Language never reaches it; the other trees do.
SDK_CHECKOUT="${DARTQ_SDK_CHECKOUT:-}"
if [ "$TREE_KIND" != "Language" ]; then
  if [ -z "$SDK_CHECKOUT" ]; then
    echo "co19-sweep: DARTQ_SDK_CHECKOUT is required for $TREE_KIND — the tree references" >&2
    echo "  package: targets resolved through <sdk-checkout>/pkg/<name> (the language-sweep" >&2
    echo "  face). Point it at a dart-lang/sdk source checkout, e.g.:" >&2
    echo "  DARTQ_SDK_CHECKOUT=<sdk-checkout> DARTQ_CO19_CORPUS=<co19>/$TREE_KIND scripts/co19-sweep.sh --dry-run" >&2
    exit 2
  fi
  if [ ! -d "$SDK_CHECKOUT/pkg" ]; then
    echo "co19-sweep: DARTQ_SDK_CHECKOUT=$SDK_CHECKOUT has no pkg/ dir — not a dart-lang/sdk source checkout" >&2
    exit 2
  fi
fi

SCAN="$CORPUS"
SCAN_DESC="co19/$TREE_KIND"
if [ -n "$ONLY" ]; then
  if [ ! -d "$CORPUS/$ONLY" ]; then
    echo "co19-sweep: --only '$ONLY' is not a directory under $CORPUS" >&2
    exit 2
  fi
  SCAN="$CORPUS/$ONLY"
  SCAN_DESC="$SCAN_DESC/$ONLY"
fi

# --- corpus statistics (read-only; also recorded pre-run in real runs) ------
# The noise/scale surface this prints is the evidence behind the global
# materialization ruling above: measure it before trusting any run.
scan_stats() { # $1 = scan root — prints the report to stdout
  local scan="$1"
  local files top_dirs part_files named_po uri_po banners banner_versions xdir utils_refs refs
  files=$(find "$scan" -name '*.dart' -type f | wc -l | tr -d ' ')
  top_dirs=$(find "$scan" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
  part_files=$(grep -rlE "^part ['\"]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  named_po=$(grep -rlE "^part of [A-Za-z_]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  uri_po=$(grep -rlE "^part of ['\"]" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  banners=$(grep -rlE "^// ?@dart *=" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  banner_versions=$(grep -rhoE "^// ?@dart *= *[0-9]+\.[0-9]+" --include='*.dart' "$scan" 2>/dev/null \
    | grep -oE '[0-9]+\.[0-9]+$' | sort -u | tr '\n' ' ' || true)
  xdir=$(grep -rhoE "(import|export|part) +['\"](\.\./)+" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)
  utils_refs=$(grep -rhoE "(import|export|part) +['\"](\.\./)+Utils/" --include='*.dart' "$scan" 2>/dev/null | wc -l | tr -d ' ' || true)

  echo "corpus: $scan"
  echo "  files: $files  top-level feature dirs: $top_dirs"
  echo "  part directive files: $part_files  (part-of named: $named_po, part-of uri: $uri_po)"
  echo "  @dart banner files: $banners  versions: ${banner_versions:-none}"
  echo "  cross-dir relative refs (../): $xdir  (into Utils/ helpers: $utils_refs)" \
    " <- global materialization keeps these in-package (staging/co19/{Language,Utils})"
  echo "  package: uri references (import/export) per target package:"
  refs=$(grep -rhoE "(import|export) +['\"]package:[A-Za-z_0-9]+/" --include='*.dart' "$scan" 2>/dev/null \
    | sed -E 's/.*package:([A-Za-z_0-9]+)\/.*/\1/' | sort | uniq -c | sort -rn || true)
  if [ -z "$refs" ]; then
    if [ "$TREE_KIND" = "Language" ]; then
      echo "    (none — expected: Language uses relative URIs only)"
    else
      echo "    (none found in this slice)"
    fi
  else
    printf '%s\n' "$refs" | while read -r count name; do
      if [ "$TREE_KIND" != "Language" ] && [ -d "$SDK_CHECKOUT/pkg/$name" ]; then
        echo "    $count  package:$name  (mapped -> <sdk-checkout>/pkg/$name)"
      else
        echo "    $count  package:$name  (UNMAPPED — unresolved identically on both sides)"
      fi
    done
  fi
}

# The helper-package preflight is tree-specific: Language needs repo-local
# ../Utils; the pkg/-mapping trees need the SDK checkout (already checked).
if [ "$TREE_KIND" = "Language" ] && [ ! -f "$CO19_ROOT/Utils/expect.dart" ]; then
  echo "co19-sweep: helper package not found at $CO19_ROOT/Utils/expect.dart" >&2
  echo "  DARTQ_CO19_CORPUS must point at the co19 repo's Language tree so the" >&2
  echo "  repo-local Utils/ helper package is reachable at ../Utils" >&2
  exit 2
fi

echo "co19-sweep: label=$LABEL corpus=$SCAN_DESC repo-root=$CO19_ROOT"
scan_stats "$SCAN"

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $sdk_desc}"
  echo "co19-sweep: DRY RUN — statistics above; real run would execute:"
  echo "  materialize: cp -Rc $SCAN -> target/evidence/co19-sweep/$LABEL/staging/$SCAN_DESC (clonefile, cp fallback)" \
    "$( [ "$TREE_KIND" = "Language" ] && echo "+ cp -Rc $CO19_ROOT/Utils -> .../staging/co19/Utils " )" \
    "; write staging/{pubspec.yaml,pubspec.lock,.dart_tool/package_config.json} (self rootUri ../ languageVersion 3.13" \
    "$( [ "$TREE_KIND" != "Language" ] && echo " + mapped <sdk-checkout>/pkg/* helper entries" )" \
    ")"
  echo "  diff: ${HARNESS_BIN:-./target/release/harness} run --corpus-dir target/evidence/co19-sweep/$LABEL/staging \\"
  echo "    --oracle-dart $sdk_desc/bin/dart --dartq-cmd \"$dartq_cmd\" --severity-filter all --allow-vacuous \\"
  echo "    --persist-regressions target/evidence/co19-sweep/$LABEL/cases --out target/evidence/co19-sweep/$LABEL/diff"
  echo "  triage: fold diff/report.json into triage.{json,md} clustered by (side, severity, code)"
  exit 0
fi

# --- real run preflight -------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "co19-sweep: DARTQ_DART_SDK is required for a real run (pinned oracle Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "co19-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "co19-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "co19-sweep: harness/dartq binaries missing after build" >&2
  exit 2
fi
DARTQ_CMD="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $DARTQ_DART_SDK}"

OUT="target/evidence/co19-sweep/$LABEL"
if [ -d "$OUT" ]; then
  echo "co19-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"

# --- face 1: manifest ----------------------------------------------------------
# Version caliber on record before anything runs (see header): co19 is a
# rolling-main corpus with no VERSION file; commit is best-effort.
CORPUS_VERSION="rolling main (co19 corpus, no VERSION file)"
if commit=$(git -C "$CO19_ROOT" rev-parse --short HEAD 2>/dev/null); then
  CORPUS_VERSION="rolling main (co19 corpus, commit $commit)"
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
# Clone with the original path depth preserved (staging/co19/Language[/only]
# plus staging/co19/Utils) so every relative URI — including the 4-level
# helper escapes — resolves exactly as in the source tree (see header).
clone_dir() { # $1 = src dir, $2 = dst dir — clonefile with plain-copy fallback
  if cp -Rc "$1/." "$2/" 2>/dev/null; then
    echo clone
  else
    cp -R "$1/." "$2/"
    echo copy
  fi
}
STAGE_ROOT="$OUT/staging"
STAGE_CORPUS="$STAGE_ROOT/$SCAN_DESC"
mkdir -p "$STAGE_CORPUS"
MATERIALIZE_MODE="corpus:$(clone_dir "$SCAN" "$STAGE_CORPUS")"
if [ "$TREE_KIND" = "Language" ]; then
  STAGE_UTILS="$(dirname "$STAGE_CORPUS")/Utils"
  mkdir -p "$STAGE_UTILS"
  MATERIALIZE_MODE="$MATERIALIZE_MODE utils:$(clone_dir "$CO19_ROOT/Utils" "$STAGE_UTILS")"
fi
printf 'materialization\t%s\n' "$MATERIALIZE_MODE" >>"$MANIFEST"

printf 'name: co19_corpus\nenvironment:\n  sdk: ^3.13.0\n' >"$STAGE_ROOT/pubspec.yaml"
printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$STAGE_ROOT/pubspec.lock"

# package_config.json: the self entry (rootUri "../", languageVersion 3.13
# — the harness preflight's floor) plus, for the pkg/-mapping trees, one
# file:// entry per referenced package: target that exists at
# <sdk-checkout>/pkg/<name> — the exact language-sweep.sh face (file://
# rootUri, no packageUri, languageVersion pinned to the corpus floor).
# Unmapped targets stay out and are quantified in stats.txt.
mkdir -p "$STAGE_ROOT/.dart_tool"
python3 - "$STAGE_ROOT" "$SDK_CHECKOUT" "$SCAN" "$TREE_KIND" <<'PY'
import json
import pathlib
import re
import sys

stage, sdk_root, scan, kind = (pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]),
                               pathlib.Path(sys.argv[3]), sys.argv[4])
packages = [{"name": "co19_corpus", "rootUri": "../", "languageVersion": "3.13"}]
if kind != "Language":
    pat = re.compile(r"""(?:import|export)\s+['"]package:([A-Za-z_0-9]+)/""")
    names = set()
    for f in scan.rglob("*.dart"):
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        names.update(pat.findall(text))
    for name in sorted(names):
        pkg = sdk_root / "pkg" / name
        if pkg.is_dir():
            packages.append({"name": name, "rootUri": pkg.resolve().as_uri(),
                             "languageVersion": "3.13"})
    print(f"package_config: {len(packages) - 1} mapped pkg/ helper package(s): "
          + ", ".join(p["name"] for p in packages[1:]))
config = {"configVersion": 2, "generator": "pub", "packages": packages}
(stage / ".dart_tool" / "package_config.json").write_text(
    json.dumps(config, indent=1) + "\n", encoding="utf-8")
PY

# --- face 3: differential -------------------------------------------------------
# harness exit codes: 0 pass, 1 divergences found (discovery — cases
# persisted), 2 harness/infra error (fatal for the sweep).
echo "co19-sweep: face 3 — differential (all severities)"
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
  echo "co19-sweep: harness error (exit 2) — see $OUT/harness.log" >&2
  exit 2
fi
echo "co19-sweep: diff done (exit $status; 1 = divergences persisted) — $OUT/harness.log"

# --- face 4: triage (code-clustered, never per-file) ----------------------------
python3 - "$OUT" "$SCAN_DESC" <<'PY'
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
    "schema": "co19-sweep-triage-v1",
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

md = [f"# Co19 sweep triage — {out.name}", "",
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
       "divergence cases (regression-case-v1). Rolling-main corpus: pair "
       "corpus_tree_version with the oracle version when attributing "
       "version-shaped clusters."]
(out / "triage.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print(f"triage: {len(clusters)} diff cluster(s) -> {out / 'triage.md'}")
PY

echo "co19-sweep: complete — evidence in $OUT (stats.txt, versions.tsv, diff/, cases/, triage.md)"
[ "$status" -eq 1 ] && echo "co19-sweep: divergences found and persisted — discoveries, attribute via triage.md clusters"
exit 0
