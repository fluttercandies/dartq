#!/usr/bin/env bash
# recovery-sweep.sh — parser-recovery corpus differential sweep (ticket #1573).
#
# Sweeps the in-repo recovery corpus (tests/corpus/recovery/, 3,757 .dart
# fixtures across 6 damage categories) through the standard differential
# harness: oracle `dart analyze` vs `dartq analyze --format=machine`, all
# severities judged, divergences persisted in the standard
# regression-case-v1 format. No bespoke diff logic — the harness owns the
# (severity, code, file) caliber. This is the serial-phase unblocker for
# the one corpus with zero runners: the README's one-line `--corpus-dir`
# recipe cannot work as written, because the harness preflights every
# corpus package — pubspec.yaml at the corpus root (tests/harness/src/
# main.rs) and a self-entry languageVersion >= 3.13 in
# .dart_tool/package_config.json (tests/harness/src/corpus.rs) — and the
# recovery corpus deliberately carries no pubspec of its own.
#
# Corpus-face scope: every fixture is a single-file, deliberately damaged
# input — one parser-recovery case transcribed from the upstream Dart SDK
# source, pkg/analyzer/test/src/fasta/recovery (missing/extra tokens,
# paired-token damage, clause-order damage, misplaced code). Expectations
# are never transcribed: what parity looks like is decided by the
# differential gate alone, the same contract as every other syntax corpus
# (the corpus README). Damaged directive lines in fixtures (part/part-of,
# 73 files) are fixture CONTENT under test, not library structure — any
# resolution or non-resolution is byte-identical on both sides, a shared
# face, never one-sided noise.
#
# Materialization ruling (global, not per-category; measured, not assumed):
#   The harness preflight requires one package root per run (pubspec.yaml +
#   .dart_tool/package_config.json with a self entry `rootUri: "../"` whose
#   languageVersion is >= 3.13). The corpus is materialized as ONE package
#   because:
#     1. Fixtures are independent single-file inputs: zero `package:` and
#        zero relative import/export/part URIs exist in the corpus
#        (measured), so no helper mapping and no sibling staging are
#        needed. One run = one oracle process + one dartq process over the
#        whole tree; per-category materialization would mean 6 process
#        pairs for zero semantic gain.
#     2. Path depth is preserved (staging/tests/corpus/recovery/...), the
#        sweep-family convention, so the staged tree reproduces the
#        in-repo layout exactly — any relative URI a future fixture
#        generation adds would resolve as in the source tree, and evidence
#        paths stay self-describing.
#   The corpus tree is NEVER written to: staging is a clone (APFS
#   clonefile, plain-copy fallback) under target/evidence/. The clone is
#   faithful (README, generate.py, __pycache__ ride along) — the harness
#   scans .dart files only, so non-Dart content is inert on both sides.
#   Symlinked subtrees were rejected: recursive directory walkers do not
#   follow symlinked dirs, which would silently scan zero files.
#
# package: resolution: none needed (zero references, measured). A single
#   self entry `rootUri: "../"` with languageVersion 3.13 (the harness
#   Plan A floor) satisfies the preflight; any stray package: target a
#   future generation introduces stays unmapped and is quantified in the
#   stats output — both sides see the identical unresolved set, so it is
#   agreed noise, not differential noise.
#
# Version caliber (parametrized): the oracle SDK is whatever DARTQ_DART_SDK
#   points at (variable). The corpus is generated in-repo (generate.py,
#   upstream recovery test transcriptions), so its "version" is the repo
#   commit it was generated at: versions.tsv records the generated-corpus
#   declaration plus the best-effort git commit of the corpus tree. The
#   run-time oracle version captured by the harness (report
#   meta.oracle_version) is the attribution truth folded into triage.md.
#   Version-drift divergences are discoveries to be attributed, not sweep
#   failures.
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
# SERIAL PHASE ONLY: the real run executes oracle + dartq over 3.7k files
# (memory-incident discipline). Never alongside development lanes:
#   DARTQ_RECOVERY_CORPUS=tests/corpus/recovery \
#   DARTQ_DART_SDK=<pinned dart sdk root> \
#     scripts/recovery-sweep.sh --label recovery-r1
#
# Usage: scripts/recovery-sweep.sh [--label L] [--dry-run] [--only <dir>]
#   --label L    evidence dir name under target/evidence/recovery-sweep/
#                (default RECOVERY_SWEEP_LABEL env, else recovery-r1). An
#                existing directory refuses to run — recorded evidence is
#                never overwritten (bump the label).
#   --dry-run    read-only noise/scale statistics over the corpus (file
#                counts per damage category, part-of damage surface,
#                package:/relative URI surface, @dart banners) plus the
#                exact planned commands; executes nothing. Requires
#                DARTQ_RECOVERY_CORPUS, not DARTQ_DART_SDK.
#   --only <dir> restrict the sweep to one damage category under the
#                corpus tree (code_order, extra_code, invalid_code,
#                missing_code, paired_tokens, partial_code — e.g.
#                --only partial_code, which carries 3,562 of the 3,757
#                fixtures). This is the mid-face crash recovery lever:
#                rerun categories one at a time with fresh labels.
# Env:   DARTQ_RECOVERY_CORPUS  path to the recovery corpus tree
#                                (required, always — the stats read it).
#        DARTQ_DART_SDK         pinned oracle Dart SDK root (>= 3.13);
#                               required unless --dry-run.
#        DARTQ_CMD              dartq side template; default
#                               "./target/release/dartq analyze
#                               --format=machine --sdk <DARTQ_DART_SDK>".
#        HARNESS_BIN            default ./target/release/harness.
set -euo pipefail
# Locale-stable byte order for the sort/uniq summary sites (precedent: scripts/incremental-gate.sh).
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

LABEL="${RECOVERY_SWEEP_LABEL:-recovery-r1}"
DRY_RUN=0
ONLY=""
CORPUS="${DARTQ_RECOVERY_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --only) ONLY="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "recovery-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if [ -z "$CORPUS" ]; then
  echo "recovery-sweep: DARTQ_RECOVERY_CORPUS is required — set it to the recovery corpus tree," >&2
  echo "  e.g. DARTQ_RECOVERY_CORPUS=tests/corpus/recovery scripts/recovery-sweep.sh --dry-run" >&2
  exit 2
fi
if [ ! -d "$CORPUS" ]; then
  echo "recovery-sweep: DARTQ_RECOVERY_CORPUS=$CORPUS is not a directory" >&2
  exit 2
fi
CORPUS="$(cd "$CORPUS" && pwd)"

# Corpus integrity probe: the recovery corpus is a generated tree (one
# generator + README at the root); a tree without them is a mispointed
# path, not this corpus.
if [ ! -f "$CORPUS/generate.py" ] || [ ! -f "$CORPUS/README.md" ]; then
  echo "recovery-sweep: $CORPUS does not look like the recovery corpus (no generate.py + README.md at the root)" >&2
  echo "  DARTQ_RECOVERY_CORPUS must point at the recovery corpus tree (tests/corpus/recovery in this repo)" >&2
  exit 2
fi

SCAN="$CORPUS"
SCAN_DESC="tests/corpus/recovery"
if [ -n "$ONLY" ]; then
  if [ ! -d "$CORPUS/$ONLY" ]; then
    echo "recovery-sweep: --only '$ONLY' is not a directory under $CORPUS" >&2
    echo "  damage categories: code_order extra_code invalid_code missing_code paired_tokens partial_code" >&2
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
  echo "  files: $files  top-level dirs: $top_dirs  (fixture categories listed below)"
  echo "  fixtures per damage category:"
  for d in "$scan"/*/; do
    n=$(find "$d" -name '*.dart' -type f | wc -l | tr -d ' ')
    if [ "$n" -gt 0 ]; then
      printf '    %6s  %s\n' "$n" "$(basename "$d")"
    fi
  done
  echo "  part directive files: $part_files  (part-of named: $named_po, part-of uri: $uri_po)" \
    "<- damaged-directive fixtures: content under test, not library structure"
  echo "  @dart banner files: $banners  versions: ${banner_versions:-none}"
  echo "  cross-dir relative refs (../): $xdir  <- global materialization keeps the staged tree at source depth"
  echo "  package: uri references (import/export) per target package:"
  refs=$(grep -rhoE "(import|export) +['\"]package:[A-Za-z_0-9]+/" --include='*.dart' "$scan" 2>/dev/null \
    | sed -E 's/.*package:([A-Za-z_0-9]+)\/.*/\1/' | sort | uniq -c | sort -rn || true)
  if [ -z "$refs" ]; then
    echo "    (none — expected: fixtures are single-file inputs)"
  else
    printf '%s\n' "$refs" | while read -r count name; do
      echo "    $count  package:$name  (UNMAPPED — no helper mapping; unresolved identically on both sides)"
    done
  fi
}

echo "recovery-sweep: label=$LABEL corpus=$SCAN_DESC"
scan_stats "$SCAN"

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $sdk_desc}"
  echo "recovery-sweep: DRY RUN — statistics above; real run would execute:"
  echo "  materialize: cp -Rc $SCAN -> target/evidence/recovery-sweep/$LABEL/staging/$SCAN_DESC (clonefile, cp fallback)" \
    "; write staging/{pubspec.yaml,pubspec.lock,.dart_tool/package_config.json} (self rootUri ../ languageVersion 3.13, no helper mapping needed)"
  echo "  diff: ${HARNESS_BIN:-./target/release/harness} run --corpus-dir target/evidence/recovery-sweep/$LABEL/staging \\"
  echo "    --oracle-dart $sdk_desc/bin/dart --dartq-cmd \"$dartq_cmd\" --severity-filter all --allow-vacuous \\"
  echo "    --persist-regressions target/evidence/recovery-sweep/$LABEL/cases --out target/evidence/recovery-sweep/$LABEL/diff"
  echo "  triage: fold diff/report.json into triage.{json,md} clustered by (side, severity, code)"
  exit 0
fi

# --- real run preflight -------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "recovery-sweep: DARTQ_DART_SDK is required for a real run (pinned oracle Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "recovery-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "recovery-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "recovery-sweep: harness/dartq binaries missing after build" >&2
  exit 2
fi
DARTQ_CMD="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $DARTQ_DART_SDK}"

OUT="target/evidence/recovery-sweep/$LABEL"
if [ -d "$OUT" ]; then
  echo "recovery-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"

# --- face 1: manifest ----------------------------------------------------------
# Version caliber on record before anything runs (see header): the corpus
# is generated in-repo; the commit that generated it is best-effort.
CORPUS_VERSION="generated corpus (recovery fixtures, generate.py)"
if commit=$(git -C "$CORPUS" rev-parse --short HEAD 2>/dev/null); then
  CORPUS_VERSION="generated corpus (recovery fixtures, generate.py, commit $commit)"
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
# Clone with the original path depth preserved (staging/tests/corpus/
# recovery[/only]) so the staged tree reproduces the in-repo layout (see
# header).
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
printf 'materialization\t%s\n' "$MATERIALIZE_MODE" >>"$MANIFEST"

printf 'name: recovery_corpus\nenvironment:\n  sdk: ^3.13.0\n' >"$STAGE_ROOT/pubspec.yaml"
printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$STAGE_ROOT/pubspec.lock"

# package_config.json: single self entry exactly `rootUri: "../"` (the
# harness languageVersion preflight matches on it). The corpus has zero
# package: references (measured) — no helper mapping face is needed; any
# stray package: target stays unmapped and is quantified in stats.txt.
mkdir -p "$STAGE_ROOT/.dart_tool"
printf '{"configVersion": 2, "generator": "pub", "packages": [{"name": "recovery_corpus", "rootUri": "../", "languageVersion": "3.13"}]}\n' \
  >"$STAGE_ROOT/.dart_tool/package_config.json"

# --- face 3: differential -------------------------------------------------------
# harness exit codes: 0 pass, 1 divergences found (discovery — cases
# persisted), 2 harness/infra error (fatal for the sweep).
echo "recovery-sweep: face 3 — differential (all severities)"
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
  echo "recovery-sweep: harness error (exit 2) — see $OUT/harness.log" >&2
  exit 2
fi
echo "recovery-sweep: diff done (exit $status; 1 = divergences persisted) — $OUT/harness.log"

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
    "schema": "recovery-sweep-triage-v1",
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

md = [f"# Recovery sweep triage — {out.name}", "",
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
       "divergence cases (regression-case-v1). Generated corpus: fixture bytes "
       "are versioned with the repo (corpus_tree_version); pair with the oracle "
       "version when attributing version-shaped clusters."]
(out / "triage.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print(f"triage: {len(clusters)} diff cluster(s) -> {out / 'triage.md'}")
PY

echo "recovery-sweep: complete — evidence in $OUT (stats.txt, versions.tsv, diff/, cases/, triage.md)"
[ "$status" -eq 1 ] && echo "recovery-sweep: divergences found and persisted — discoveries, attribute via triage.md clusters"
exit 0
