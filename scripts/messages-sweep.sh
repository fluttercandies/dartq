#!/usr/bin/env bash
# messages-sweep.sh — official messages.yaml example-corpus differential sweep
# (ticket #1574).
#
# Sweeps the per-code example corpus (876 fixtures: 650 analyzer-plane, 226
# linter-plane, transcribed from the official diagnostic documentation —
# upstream Dart SDK source, pkg/analyzer/messages.yaml and
# pkg/linter/messages.yaml) through the standard differential harness:
# oracle `dart analyze` vs `dartq analyze --format=machine`.
#
# Judging axis (ticket-core design decision): per-code PRESENCE alignment,
# not a raw (severity, code, file) set-diff alone.
#   Every fixture is a documented reproducer for exactly one diagnostic code
#   (the upstream verifier enforces that each documented example, analyzed
#   on its own, produces exactly that code), so the corpus's value is the
#   per-fixture question "does the documented code actually fire on its own
#   example, on each side?" A plain triple set-diff cannot express the two
#   failure shapes unique to this corpus:
#     - NEITHER side emits the documented code (the fixture does not
#       reproduce under this oracle/SDK build: version semantics, or
#       example syntax gated on experiments the run does not enable —
#       census.txt counts the stripped %language/%experiments directives).
#       Set-diff scores both-empty as a match and hides it entirely.
#     - the documented code fires but among extra diagnostics — that is
#       per-fixture attribution, not an aggregate rate.
#   Therefore: one harness run (single oracle + dartq process pair, standard
#   caliber, divergences persisted as usual) + post-hoc per-fixture
#   classification from raw-outputs.json — the harness's own complete,
#   unfiltered per-side plane — into both / oracle-only / dartq-only /
#   neither per fixture, rolled up per expected code, with
#   presence_jaccard = both / (both + oracle-only + dartq-only) as the
#   headline metric. "neither" is corpus/SDK-version semantics, not
#   differential disagreement, and stays out of the denominator. The
#   harness diff clusters (report.json oracle_only/dartq_only by
#   (severity, code)) are kept verbatim as the complementary set-diff face:
#   they catch severity divergences and extra-code noise that presence
#   alone would fold away.
#   Expected code per fixture: the emitted diagnostic name is snake_case of
#   (sharedName ?? yaml key), which is NOT always the fixture stem (56
#   renamed entries in census.txt, e.g. key ambiguousExtensionMemberAccessTwo
#   emits ambiguous_extension_member_access). The expected code is therefore
#   derived from the fixture's provenance first line (yaml class + key)
#   joined against the census.txt sharedName map. A fixture whose stem
#   disagrees with its provenance is corpus drift -> exit 2 (regenerate via
#   the corpus's generate.py).
#
# Linter-plane ruling: lint codes fire only when their rule is enabled. The
#   source corpus carries no analysis_options by design; a plain run would
#   leave both sides at zero lints and the whole 226-fixture plane
#   vacuous-agreed. Staging writes linter/analysis_options.yaml enabling
#   exactly the corpus-derived rule set (the linter plane's expected codes,
#   sorted): deterministic, regenerated per run, SDK-version-agnostic — a
#   rule unknown to the oracle SDK surfaces as an analysis-options warning
#   on BOTH sides (agreed noise face; a dartq disagreement there is a real
#   registry-face finding). The options live in the linter/ subdir only, so
#   the analyzer plane stays a pure semantic-code face (650 fixtures would
#   otherwise drown under 226 enabled lint rules). Both engines see
#   identical bytes and identical options.
#
# Materialization ruling: the harness preflight requires one package root
#   (pubspec.yaml + .dart_tool/package_config.json with a self entry
#   `rootUri: "../"`, languageVersion >= 3.13). The corpus deliberately
#   carries no pubspec — fixtures are analyzed as loose files. Staging is a
#   clone (APFS clonefile, plain-copy fallback) under target/evidence/ with
#   the synthesized package files at the staging root; the corpus tree is
#   NEVER written to. Per-fixture staging was rejected: 876 process pairs
#   is the anti-pattern the sweep family exists to avoid; one package over
#   both planes = one oracle process + one dartq process.
#
# Agreed-noise faces (identical bytes to both engines, quantified in
#   stats.txt, clustered as extras — never differential noise):
#   - package: imports (meta, flutter, p, ...) stay unmapped: no
#     package_config mapping is synthesized, so both sides resolve them to
#     nothing identically.
#   - relative imports whose targets are not in the corpus (a.dart,
#     part.dart, src/a.dart — the upstream %uri auxiliary blocks are not
#     extracted): unresolved identically on both sides.
#   - diagnostics the per-code join cannot attribute to a fixture file are
#     counted in triage as unattributed (expected: zero).
#
# Divergences are discoveries, not sweep failures: the sweep exits 0 with
#   them persisted and classified. Exit 2 = preflight, corpus-integrity, or
#   harness/infra error.
#
# SERIAL PHASE ONLY: the real run executes oracle + dartq over the corpus
# (memory-incident discipline). Never alongside development lanes:
#   DARTQ_MESSAGES_CORPUS=tests/corpus/messages-examples \
#   DARTQ_DART_SDK=<pinned dart sdk root> \
#     scripts/messages-sweep.sh --label messages-r1
#
# Usage: scripts/messages-sweep.sh [--label L] [--dry-run]
#   --label L    evidence dir name under target/evidence/messages-sweep/
#                (default MESSAGES_SWEEP_LABEL env, else messages-r1). An
#                existing directory refuses to run — recorded evidence is
#                never overwritten (bump the label).
#   --dry-run    read-only noise/scale statistics over the corpus (fixture
#                counts per plane, variants, sharedName renames, marker
#                anomalies, import surface) plus the exact planned commands;
#                executes nothing. Requires DARTQ_MESSAGES_CORPUS, not
#                DARTQ_DART_SDK.
#   No --only subset flag by design: 876 fixtures are a single cheap run;
#   the recovery levers are --dry-run (preflight) and the label bump
#   (re-run), matching the sweep family's no-checkpoint posture.
# Env:   DARTQ_MESSAGES_CORPUS  path to the messages-examples corpus root
#                          (the directory holding analyzer/, linter/,
#                          census.txt; required, always).
#        DARTQ_DART_SDK     pinned oracle Dart SDK root (>= 3.13);
#                           required unless --dry-run.
#        DARTQ_CMD          dartq side template; default
#                           "./target/release/dartq analyze
#                           --format=machine --sdk <DARTQ_DART_SDK>".
#        HARNESS_BIN        default ./target/release/harness.
set -euo pipefail

# Locale-stable byte semantics for every grep/wc/sort site (precedent:
# scripts/incremental-gate.sh); the python faces sort natively.
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

LABEL="${MESSAGES_SWEEP_LABEL:-messages-r1}"
DRY_RUN=0
CORPUS="${DARTQ_MESSAGES_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "messages-sweep: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if [ -z "$CORPUS" ]; then
  echo "messages-sweep: DARTQ_MESSAGES_CORPUS is required — set it to the messages-examples" >&2
  echo "  corpus root (the directory holding analyzer/, linter/ and census.txt)," >&2
  echo "  e.g. DARTQ_MESSAGES_CORPUS=tests/corpus/messages-examples scripts/messages-sweep.sh --dry-run" >&2
  exit 2
fi
if [ ! -d "$CORPUS" ]; then
  echo "messages-sweep: DARTQ_MESSAGES_CORPUS=$CORPUS is not a directory" >&2
  exit 2
fi
CORPUS="$(cd "$CORPUS" && pwd)"
if [ ! -f "$CORPUS/census.txt" ]; then
  echo "messages-sweep: census.txt not found under $CORPUS" >&2
  echo "  the expected-code derivation joins the corpus's census.txt sharedName map;" >&2
  echo "  regenerate the corpus via its generate.py first" >&2
  exit 2
fi
for plane in analyzer linter; do
  if [ ! -d "$CORPUS/$plane" ]; then
    echo "messages-sweep: plane directory not found: $CORPUS/$plane" >&2
    exit 2
  fi
done

# --- corpus statistics (read-only; also recorded pre-run in real runs) ------
# The noise/scale surface behind the rulings in the header: measure it before
# trusting any run.
scan_stats() { # $1 = corpus root — prints the report to stdout
  local root="$1" plane files total=0 variants renames anomalies import_files pkg_files targets missing
  echo "corpus: $root"
  for plane in analyzer linter; do
    files=$(find "$root/$plane" -name '*.dart' -type f | wc -l | tr -d ' ')
    variants=$(find "$root/$plane" -name '*_v[0-9]*.dart' -type f | wc -l | tr -d ' ')
    total=$((total + files))
    echo "  $plane: $files fixtures ($variants variant files; $((files - variants)) documented codes with a fixture)"
  done
  echo "  total fixtures: $total"
  renames=$(grep -cE '^    [A-Za-z]+\.[A-Za-z0-9]+ -> [a-z0-9_]+$' "$root/census.txt" || true)
  anomalies=$(grep -cE 'range starts / .* range ends in one example block|empty after stripping' "$root/census.txt" || true)
  echo "  census: $renames sharedName renames (expected code != fixture stem), $anomalies marker anomalies"
  for plane in analyzer linter; do
    import_files=$(grep -rlE "^(import|export|part) " --include='*.dart' "$root/$plane" 2>/dev/null | wc -l | tr -d ' ')
    pkg_files=$(grep -rl "package:" --include='*.dart' "$root/$plane" 2>/dev/null | wc -l | tr -d ' ')
    # Bare-relative directive targets (not dart:/package:), checked for
    # existence in their plane dir: absent targets resolve to nothing on
    # both sides identically (the upstream %uri auxiliary blocks are not
    # extracted — corpus README).
    targets=$(grep -rhoE "^(import|export|part) +['\"][A-Za-z_][^'\"]*" --include='*.dart' "$root/$plane" 2>/dev/null \
      | sed -E "s/.*['\"]//" | grep -v '^dart:' | LC_ALL=C sort -u)
    missing=0
    for t in $targets; do
      [ -e "$root/$plane/$t" ] || missing=$((missing + 1))
    done
    echo "  $plane import surface: $import_files files with directives; package: refs in $pkg_files files (unmapped — agreed noise); bare-relative targets: $(printf '%s\n' "$targets" | grep -c . || true) distinct, $missing absent from the corpus (unresolved identically on both sides)"
  done
}

echo "messages-sweep: label=$LABEL corpus=$CORPUS"
scan_stats "$CORPUS"

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_cmd="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $sdk_desc}"
  echo "messages-sweep: DRY RUN — statistics above; real run would execute:"
  echo "  materialize: cp -Rc $CORPUS -> target/evidence/messages-sweep/$LABEL/staging/messages-examples (clonefile, cp fallback)" \
    "; write staging/{pubspec.yaml,pubspec.lock,.dart_tool/package_config.json} (self rootUri ../ languageVersion 3.13)" \
    "; derive the linter plane's rule set from the fixtures + census sharedName map and write staging/messages-examples/linter/analysis_options.yaml" \
    "; build fixture-manifest.json (per-fixture expected code from provenance + census)"
  echo "  diff: ${HARNESS_BIN:-./target/release/harness} run --corpus-dir target/evidence/messages-sweep/$LABEL/staging \\"
  echo "    --oracle-dart $sdk_desc/bin/dart --dartq-cmd \"$dartq_cmd\" --severity-filter all --allow-vacuous \\"
  echo "    --persist-regressions target/evidence/messages-sweep/$LABEL/cases --out target/evidence/messages-sweep/$LABEL/diff"
  echo "  reconcile: fold diff/raw-outputs.json against fixture-manifest.json into per-code.tsv" \
    "(both / oracle-only / dartq-only / neither per fixture) + triage.{json,md} with presence_jaccard," \
    "keeping the harness diff clusters as the complementary set-diff face"
  exit 0
fi

# --- real run preflight -------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  echo "messages-sweep: DARTQ_DART_SDK is required for a real run (pinned oracle Dart SDK root, >= 3.13)" >&2
  exit 2
fi
export PATH="$DARTQ_DART_SDK/bin:$PATH"
if ! command -v dart >/dev/null 2>&1; then
  echo "messages-sweep: dart not found under $DARTQ_DART_SDK/bin" >&2
  exit 2
fi
HARNESS_BIN="${HARNESS_BIN:-./target/release/harness}"
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "messages-sweep: building harness + dartq-cli (release)"
  cargo build --release -p harness -p dartq-cli --quiet
fi
if [ ! -x "$HARNESS_BIN" ] || [ ! -x ./target/release/dartq ]; then
  echo "messages-sweep: harness/dartq binaries missing after build" >&2
  exit 2
fi
DARTQ_CMD="${DARTQ_CMD:-./target/release/dartq analyze --format=machine --sdk $DARTQ_DART_SDK}"

OUT="target/evidence/messages-sweep/$LABEL"
if [ -d "$OUT" ]; then
  echo "messages-sweep: $OUT already exists — recorded evidence is never overwritten, bump --label" >&2
  exit 2
fi
mkdir -p "$OUT"

# --- face 1: manifest ----------------------------------------------------------
# Version caliber on record before anything runs (see header): the corpus is
# generated documentation transcription with no VERSION file; the census
# totals line is its identity stamp.
CENSUS_TOTALS="$(grep '^total fixtures:' "$CORPUS/census.txt" || echo 'total fixtures: unknown')"
CORPUS_VERSION="generated corpus ($CENSUS_TOTALS; regenerated deterministically by its generate.py)"
MANIFEST="$OUT/versions.tsv"
{
  printf 'key\tvalue\n'
  printf 'oracle_sdk\t%s\n' "$DARTQ_DART_SDK"
  printf 'corpus_tree\t%s\n' "$CORPUS"
  printf 'corpus_tree_version\t%s\n' "$CORPUS_VERSION"
  printf 'corpus_language_version\t3.13 (package_config pin; per-example %%language/%%experiments directive context is intentionally not baked into fixtures — census.txt counts them)\n'
  printf 'dartq_cmd\t%s\n' "$DARTQ_CMD"
} >"$MANIFEST"
scan_stats "$CORPUS" >"$OUT/stats.txt"
cat "$MANIFEST"

# --- face 2: materialization ----------------------------------------------------
# Clone the corpus (never written to), synthesize the package files at the
# staging root, derive the linter rule set + per-fixture expected codes into
# fixture-manifest.json. One python pass keeps derivation deterministic and
# the integrity gate (provenance vs stem) in one place.
clone_dir() { # $1 = src dir, $2 = dst dir — clonefile with plain-copy fallback
  if cp -Rc "$1/." "$2/" 2>/dev/null; then
    echo clone
  else
    cp -R "$1/." "$2/"
    echo copy
  fi
}
STAGE_ROOT="$OUT/staging"
STAGE_CORPUS="$STAGE_ROOT/messages-examples"
mkdir -p "$STAGE_CORPUS"
MATERIALIZE_MODE="corpus:$(clone_dir "$CORPUS" "$STAGE_CORPUS")"
printf 'materialization\t%s\n' "$MATERIALIZE_MODE" >>"$MANIFEST"

printf 'name: messages_corpus\nenvironment:\n  sdk: ^3.13.0\n' >"$STAGE_ROOT/pubspec.yaml"
printf 'packages: {}\nsdks:\n  dart: ">=3.13.0 <4.0.0"\n' >"$STAGE_ROOT/pubspec.lock"

# package_config.json: single self entry exactly `rootUri: "../"` (the
# harness languageVersion preflight matches on it). No package: mapping is
# synthesized: unmapped package imports resolve identically on both sides.
mkdir -p "$STAGE_ROOT/.dart_tool"
printf '{"configVersion": 2, "generator": "pub", "packages": [{"name": "messages_corpus", "rootUri": "../", "languageVersion": "3.13"}]}\n' \
  >"$STAGE_ROOT/.dart_tool/package_config.json"

python3 - "$CORPUS" "$STAGE_ROOT" "$OUT" <<'PY'
import json
import pathlib
import re
import sys

corpus, stage_root, out = (pathlib.Path(p) for p in sys.argv[1:4])

# camel_to_snake mirrors the corpus generator exactly: fixture stems are the
# yaml keys converted with this algorithm.
def camel_to_snake(name: str) -> str:
    parts = []
    for i, ch in enumerate(name):
        if ch.isupper() and i > 0 and (not name[i - 1].isupper() or (i + 1 < len(name) and name[i + 1].islower())):
            parts.append("_")
        parts.append(ch.lower())
    return "".join(parts)

# The census records codes whose emitted diagnostic name (snake_case of
# sharedName ?? key) differs from the fixture stem:
#     <Class>.<key> -> <emitted>
renamed = {}
for line in (corpus / "census.txt").read_text(encoding="utf-8").splitlines():
    m = re.fullmatch(r"\s+([A-Za-z]+)\.([A-Za-z0-9]+) -> ([a-z0-9_]+)\s*", line)
    if m:
        renamed[f"{m.group(1)}.{m.group(2)}"] = m.group(3)

prov_re = re.compile(
    r"^// messages-example: (analyzer|linter)/messages\.yaml "
    r"([A-Za-z]+)\.([A-Za-z0-9]+)(?: \(variant \d+\))?$"
)

fixtures = {}
rules = set()
drift = []
for plane in ("analyzer", "linter"):
    for f in sorted((stage_root / "messages-examples" / plane).glob("*.dart")):
        first = f.read_text(encoding="utf-8").split("\n", 1)[0].rstrip("\r")
        m = prov_re.match(first)
        if not m:
            drift.append(f"{plane}/{f.name}: provenance line unparsable: {first!r}")
            continue
        cls, key = m.group(2), m.group(3)
        stem = camel_to_snake(key)
        base = re.sub(r"_v\d+$", "", f.stem)
        if stem != base:
            drift.append(f"{plane}/{f.name}: stem {base!r} != provenance key snake {stem!r}")
            continue
        expected = renamed.get(f"{cls}.{key}", stem)
        # Key = the harness entry-key file form: path relative to the corpus
        # dir passed to `harness run` (the staging root), posix, no label
        # prefix (single-corpus run) — this is what raw-outputs.json joins on.
        fixtures[f"messages-examples/{plane}/{f.name}"] = {
            "plane": plane,
            "qualified": f"{cls}.{key}",
            "expected": expected,
        }
        if plane == "linter":
            rules.add(expected)

if drift:
    print("messages-sweep: corpus integrity drift (regenerate via the corpus's generate.py):", file=sys.stderr)
    for d in drift[:5]:
        print(f"  {d}", file=sys.stderr)
    if len(drift) > 5:
        print(f"  ... and {len(drift) - 5} more", file=sys.stderr)
    raise SystemExit(2)

if not fixtures:
    print("messages-sweep: no fixtures found under the staging planes", file=sys.stderr)
    raise SystemExit(2)

# Linter-plane ruling (header): enable exactly the corpus-derived rule set,
# scoped to the linter/ subdir so the analyzer plane stays lint-free.
options = stage_root / "messages-examples" / "linter" / "analysis_options.yaml"
if rules:
    body = "".join(f"    - {r}\n" for r in sorted(rules))
    options.write_text(
        "# Generated by scripts/messages-sweep.sh staging: the linter plane's\n"
        "# corpus-derived rule set (the plane's expected diagnostic codes), so\n"
        "# the documented lint examples actually fire on both sides. Scoped to\n"
        "# this directory; regenerated per run; never written to the source\n"
        "# corpus.\n"
        "linter:\n  rules:\n" + body,
        encoding="utf-8",
    )

manifest = {
    "fixtures": fixtures,
    "linter_rules": sorted(rules),
    "counts": {
        "fixtures": len(fixtures),
        "analyzer": sum(1 for v in fixtures.values() if v["plane"] == "analyzer"),
        "linter": sum(1 for v in fixtures.values() if v["plane"] == "linter"),
        "distinct_expected_codes": len({v["expected"] for v in fixtures.values()}),
        "renamed_entries": len(renamed),
    },
}
(out / "fixture-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
printf 'linter_rules_enabled\t%s (staging messages-examples/linter/analysis_options.yaml, corpus-derived)\n' \
  "$(python3 -c 'import json,sys; print(len(json.loads(open(sys.argv[1]).read())["linter_rules"]))' "$OUT/fixture-manifest.json")" >>"$MANIFEST"
printf 'fixture_manifest\t%s\n' \
  "$(python3 -c 'import json,sys; print(json.loads(open(sys.argv[1]).read())["counts"]["fixtures"])' "$OUT/fixture-manifest.json")" >>"$MANIFEST"

# --- face 3: differential -------------------------------------------------------
# harness exit codes: 0 pass, 1 divergences found (discovery — cases
# persisted), 2 harness/infra error (fatal for the sweep).
echo "messages-sweep: face 3 — differential (all severities)"
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
  echo "messages-sweep: harness error (exit 2) — see $OUT/harness.log" >&2
  exit 2
fi
echo "messages-sweep: diff done (exit $status; 1 = divergences persisted) — $OUT/harness.log"

# --- face 4: per-code reconciliation --------------------------------------------
# The judging axis (header): classify every fixture by presence of its
# documented code on each side, roll up per expected code, and keep the
# harness diff clusters as the complementary set-diff face.
python3 - "$OUT" <<'PY'
import collections
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
man = json.loads((out / "fixture-manifest.json").read_text(encoding="utf-8"))
fixtures = man["fixtures"]
rep = json.loads((out / "diff" / "report.json").read_text(encoding="utf-8"))
raw = json.loads((out / "diff" / "raw-outputs.json").read_text(encoding="utf-8"))

present = {side: collections.defaultdict(set) for side in ("oracle", "dartq")}
extras = {side: collections.defaultdict(set) for side in ("oracle", "dartq")}
unattributed = collections.Counter()
for side in ("oracle", "dartq"):
    for corpus in raw[side]["corpora"]:
        for d in corpus["diagnostics"]:
            f, code = d["file"], d["code"]
            fx = fixtures.get(f)
            if fx is None:
                unattributed[side] += 1
            elif code == fx["expected"]:
                present[side][f].add(code)
            else:
                extras[side][f].add(code)

CLASS_ORDER = ("both", "oracle-only", "dartq-only", "neither")
rows = []
for f in sorted(fixtures):
    fx = fixtures[f]
    o, q = f in present["oracle"], f in present["dartq"]
    cls = ("both" if o and q else "oracle-only" if o else "dartq-only" if q else "neither")
    rows.append({
        "plane": fx["plane"],
        "fixture": f,
        "qualified": fx["qualified"],
        "expected_code": fx["expected"],
        "class": cls,
        "oracle_extra": ",".join(sorted(extras["oracle"][f])),
        "dartq_extra": ",".join(sorted(extras["dartq"][f])),
    })

# per-code.tsv: the per-fixture attribution face (machine + spreadsheet).
tsv = out / "per-code.tsv"
with tsv.open("w", encoding="utf-8") as fh:
    fh.write("plane\tfixture\tqualified\texpected_code\tclass\toracle_extra\tdartq_extra\n")
    for r in rows:
        fh.write("\t".join(r[k] for k in ("plane", "fixture", "qualified", "expected_code", "class", "oracle_extra", "dartq_extra")) + "\n")

def class_counts(plane):
    c = collections.Counter(r["class"] for r in rows if r["plane"] == plane)
    return {k: c.get(k, 0) for k in CLASS_ORDER}

def presence_jaccard(c):
    denom = c["both"] + c["oracle-only"] + c["dartq-only"]
    return round(c["both"] / denom, 6) if denom else None

planes = {
    p: {"counts": class_counts(p),
        "presence_jaccard": presence_jaccard(class_counts(p))}
    for p in ("analyzer", "linter")
}
overall_counts = collections.Counter(r["class"] for r in rows)
overall = {k: overall_counts.get(k, 0) for k in CLASS_ORDER}

# Rollup per expected code: the per-code reconciliation surface (a code whose
# every fixture is "both" is aligned; anything else is discovery surface).
codes = collections.defaultdict(lambda: collections.Counter())
for r in rows:
    codes[r["expected_code"]][r["class"]] += 1
codes_out = {
    code: {"fixtures": sum(c.values()), **{k: c.get(k, 0) for k in CLASS_ORDER}}
    for code, c in sorted(codes.items())
}
misaligned = [c for c, v in codes_out.items() if v["both"] != v["fixtures"]]

# Complementary set-diff face, verbatim sweep-family clusters (severity, code).
clusters = []
for side, side_rows in (("oracle-only", rep.get("oracle_only") or []),
                        ("dartq-only", rep.get("dartq_only") or [])):
    grouped = collections.defaultdict(lambda: {"entries": 0, "instances": 0, "files": set()})
    for row in side_rows:
        g = grouped[(row.get("severity", ""), row.get("code", ""))]
        g["entries"] += 1
        g["instances"] += row.get("count", 0)
        g["files"].add(row.get("file", ""))
    for (severity, code), g in sorted(grouped.items()):
        clusters.append({"side": side, "severity": severity, "code": code,
                         "entries": g["entries"], "instances": g["instances"],
                         "files": len(g["files"])})
clusters.sort(key=lambda c: -c["instances"])

summary = {
    "schema": "messages-sweep-triage-v1",
    "judging_axis": "per-code presence (both/oracle-only/dartq-only/neither per fixture) "
                    "+ harness (severity, code) diff clusters as the complementary set-diff face",
    "oracle_version": (rep.get("meta") or {}).get("oracle_version", ""),
    "corpus_counts": man["counts"],
    "passed": rep.get("passed"),
    "error_jaccard": rep.get("error_jaccard"),
    "file_count": rep.get("file_count"),
    "oracle_total": rep.get("oracle_total"),
    "dartq_total": rep.get("dartq_total"),
    "severities": [{"severity": s["severity"], "jaccard": s["jaccard"]}
                   for s in rep.get("severities", [])],
    "overall": {"counts": overall, "presence_jaccard": presence_jaccard(overall)},
    "planes": planes,
    "codes": codes_out,
    "misaligned_codes": misaligned,
    "unattributed_diagnostics": dict(unattributed),
    "clusters": clusters,
}
(out / "triage.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")

# triage.md: headline reconciliation + discovery surface; per-fixture detail
# lives in per-code.tsv, full machine face in triage.json.
MD_CAP = 60
md = [f"# Messages-examples sweep triage — {out.name}", "",
      f"fixtures: {man['counts']['fixtures']} "
      f"(analyzer {man['counts']['analyzer']}, linter {man['counts']['linter']}) | "
      f"oracle: {summary['oracle_version']} | language pin: 3.13", "",
      f"per-code presence — overall: {overall['both']} both / {overall['oracle-only']} oracle-only / "
      f"{overall['dartq-only']} dartq-only / {overall['neither']} neither; "
      f"presence_jaccard {summary['overall']['presence_jaccard']}", ""]
for p, v in planes.items():
    c = v["counts"]
    md.append(f"- {p}: {c['both']} both / {c['oracle-only']} oracle-only / "
              f"{c['dartq-only']} dartq-only / {c['neither']} neither "
              f"(presence_jaccard {v['presence_jaccard']})")
md += ["",
       f"harness set-diff face: oracle {summary['oracle_total']} diags, dartq {summary['dartq_total']} diags, "
       f"ERROR jaccard {summary['error_jaccard']}, passed: {summary['passed']}", "",
       "Codes with any non-both fixture (discovery surface, sorted; "
       f"{len(misaligned)} of {len(codes_out)} codes):", "",
       "| code | fixtures | both | oracle-only | dartq-only | neither |",
       "| --- | --- | --- | --- | --- | --- |"]
shown = sorted(misaligned, key=lambda c: (-codes_out[c]["oracle-only"] - codes_out[c]["dartq-only"], c))
for code in shown[:MD_CAP]:
    v = codes_out[code]
    md.append(f"| {code} | {v['fixtures']} | {v['both']} | {v['oracle-only']} | {v['dartq-only']} | {v['neither']} |")
if len(shown) > MD_CAP:
    md.append(f"| ... and {len(shown) - MD_CAP} more — see triage.json codes / per-code.tsv | | | | | |")
md += ["",
       "neither = the documented code fires on NEITHER side under this oracle/SDK build "
       "(corpus/version semantics, e.g. experiment-gated example syntax) — out of the "
       "presence_jaccard denominator by design; attribute by pairing oracle_version with "
       "the census marker/experiment notes.", ""]
if any(unattributed.values()):
    md += [f"unattributed diagnostics (files outside the fixture manifest): {dict(unattributed)}", ""]
md += ["Diff entry clusters (side, severity, code):", "",
       "| side | severity | code | entries | instances | files |",
       "| --- | --- | --- | --- | --- | --- |"]
for c in clusters:
    md.append("| {side} | {severity} | {code} | {entries} | {instances} | {files} |".format(**c))
md += ["",
       "Caliber: per-fixture attribution in per-code.tsv; full machine face in triage.json; "
       "divergence cases in cases/ (regression-case-v1); raw per-side outputs in "
       "diff/raw-outputs.json. Linter plane ran with the corpus-derived rule set staged in "
       "messages-examples/linter/analysis_options.yaml (see versions.tsv)."]
(out / "triage.md").write_text("\n".join(md) + "\n", encoding="utf-8")
print(f"reconcile: {len(rows)} fixtures -> {overall['both']} both / {overall['oracle-only']} oracle-only / "
      f"{overall['dartq-only']} dartq-only / {overall['neither']} neither; presence_jaccard "
      f"{summary['overall']['presence_jaccard']}; {len(clusters)} diff cluster(s) -> {out / 'triage.md'}")
PY

echo "messages-sweep: complete — evidence in $OUT (stats.txt, versions.tsv, fixture-manifest.json, diff/, cases/, per-code.tsv, triage.md)"
[ "$status" -eq 1 ] && echo "messages-sweep: divergences found and persisted — discoveries, attribute via triage.md + per-code.tsv"
exit 0
