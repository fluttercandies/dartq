#!/usr/bin/env bash
# token-cost-bench.sh — Output Token Cost harness: the dartq verbosity tiers
# vs the official analyzer's machine format under one tokenizer caliber
# (AI-audit finding G5; the evidence path for the P4 token-economy release
# gate and the benchmark charter's Output Token Cost metric).
#
# What it measures
#
# For one corpus (a directory of .dart files) and one fixed single-error
# fixture, the harness captures the full stdout of every leg below and
# counts tokens with one deterministic caliber, uniform across legs:
#
#   corpus (a staged clone — the original tree is never written):
#     compact : dartq analyze --format=machine --verbosity concise
#     parity  : dartq analyze --format=machine --verbosity default
#               (the byte-parity oracle row shape; the intra-tool reference)
#     full    : dartq analyze --format=machine --verbosity verbose
#     json    : dartq analyze --format=json
#     oracle  : dart analyze --format=machine
#
#   fixture (tests/bench/token-cost/single-error — exactly one diagnostic):
#     the same five legs through the `check` face (scan+parse; the release
#     gate's wording names `check`; needs no package context), with the
#     oracle leg via `dart analyze --format=machine`.
#
# Tier semantics: `concise` keeps the identifying severity/type/code+span
# prefix and drops the message (the compact token tier), `default` is the
# byte-parity oracle row, `verbose` appends the correctionMessage — the
# dartq CLI documents this axis as the token-cost sweep's tier axis.
#
# Caliber: the benchmark charter leaves the tokenizer unpinned, so the
# harness uses a deterministic character-level approximation —
# tokens = ceil(code points / CHARS_PER_TOKEN), uniform on both sides.
# CHARS_PER_TOKEN is env-parameterized (default 4) and recorded in every
# output file: the saving ratios cancel it, the absolute G1 gate does not.
# Byte counts are recorded alongside, and raw outputs are captured
# verbatim, so any other tokenizer can be re-applied offline.
#
# Gates (judged on the fixture, PASS/FAIL lines in summary.md/json):
#   G1: the fixture's compact output is <= 120 tokens.
#   G2: the fixture's compact output saves >= 50% vs the oracle machine
#       output for the same single-diagnostic content.
#   The fixture must yield exactly one compact row — anything else is a
#   fixture-integrity error (exit 2), not a gate verdict.
# Corpus legs gate nothing: they are the per-tier distribution surface
# (tokens, tokens/diag, median, P95, saving vs oracle) behind the
# benchmark charter's median/P95 metric shape.
#
# Outputs under target/evidence/token-cost/<label>/ (recorded evidence is
# never overwritten — bump the label): versions.tsv, stats.txt,
# files.manifest, staging/, raw/<leg>.{txt,err,exit}, tokens.tsv,
# tokens-dist.tsv (per-diagnostic distribution), summary.json, summary.md.
# Distribution conventions: median = lower middle of the sorted per-diagnostic
# token counts; P95 = the element at 1-based index ceil(0.95*n).
#
# Exit codes: 0 all gates pass; 1 a gate fails (results still recorded);
# 2 preflight, fixture-integrity, or counting-infrastructure error — an
# infra failure, never a gate verdict. Diverging diagnostic SETS between
# dartq and the oracle are not this harness's concern (that is the
# differential harness's judgment); only output volume is measured here.
#
# SERIAL PHASE ONLY: a real run executes the oracle and dartq over the
# corpus (memory-incident discipline — never alongside development lanes;
# one analysis process at a time; bound very large trees with --max-files).
#
# Usage:
#   DARTQ_TOKEN_CORPUS=<corpus-dir> DARTQ_DART_SDK=<sdk-root> \
#     scripts/token-cost-bench.sh --label token-r1
#   scripts/token-cost-bench.sh --dry-run --max-files 200
#
#   --label L      evidence dir name under target/evidence/token-cost/
#                  (default TOKEN_COST_LABEL env, else token-r1). An
#                  existing directory refuses to run.
#   --dry-run      corpus stats + the exact planned legs; executes nothing.
#                  Needs DARTQ_TOKEN_CORPUS but not the SDK or binaries.
#   --max-files N  measure the first N .dart files in LC_ALL=C sorted path
#                  order (deterministic subset selector). Relative imports
#                  escaping the subset resolve to nothing on both sides
#                  equally — agreed noise for a volume measurement.
# Env:
#   DARTQ_TOKEN_CORPUS           corpus directory (required, always).
#   DARTQ_DART_SDK               oracle Dart SDK root (required unless
#                                --dry-run).
#   DARTQ_BIN                    dartq binary (default
#                                ./target/release/dartq; built if missing).
#   TOKEN_COST_CHARS_PER_TOKEN   the caliber divisor (default 4; positive
#                                integer).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

FIXTURE="tests/bench/token-cost/single-error"
LABEL="${TOKEN_COST_LABEL:-token-r1}"
CPT="${TOKEN_COST_CHARS_PER_TOKEN:-4}"
MAX_FILES=""
DRY_RUN=0
CORPUS="${DARTQ_TOKEN_CORPUS:-}"

usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --label)
      [ $# -ge 2 ] || { echo "token-cost-bench: --label needs a value" >&2; exit 2; }
      LABEL="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --max-files)
      [ $# -ge 2 ] || { echo "token-cost-bench: --max-files needs a value" >&2; exit 2; }
      MAX_FILES="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "token-cost-bench: unknown option '$1' (see --help)" >&2; exit 2 ;;
  esac
done

die() { echo "token-cost-bench: $1" >&2; exit 2; }

case "$CPT" in ''|*[!0-9]*|0) die "TOKEN_COST_CHARS_PER_TOKEN must be a positive integer, got '$CPT'" ;; esac
if [ -n "$MAX_FILES" ]; then
  case "$MAX_FILES" in ''|*[!0-9]*|0) die "--max-files must be a positive integer, got '$MAX_FILES'" ;; esac
fi

[ -n "$CORPUS" ] || die "DARTQ_TOKEN_CORPUS is required — set it to a directory of .dart files, e.g. DARTQ_TOKEN_CORPUS=<corpus-dir> scripts/token-cost-bench.sh --dry-run"
[ -d "$CORPUS" ] || die "DARTQ_TOKEN_CORPUS=$CORPUS is not a directory"
CORPUS="$(cd "$CORPUS" && pwd)"
case "$CORPUS" in
  "$REPO_ROOT"/target/evidence/token-cost/*)
    die "corpus lives inside the harness evidence root: $CORPUS" ;;
esac

OUT="target/evidence/token-cost/$LABEL"
if [ -d "$OUT" ]; then
  die "$OUT already exists — recorded evidence is never overwritten, bump --label"
fi

# --- corpus manifest + scale -------------------------------------------------
# The manifest is the measured file set: LC_ALL=C sorted relative paths,
# capped by --max-files when set. Every stat and the staged clone derive
# from it, so capped and uncapped runs share one caliber. A dry run keeps
# its manifest in a temp dir — zero residue in the evidence root.
if [ "$DRY_RUN" -eq 1 ]; then
  WORK=$(mktemp -d)
  trap 'rm -rf "$WORK"' EXIT
  MANIFEST="$WORK/files.manifest"
else
  mkdir -p "$OUT"
  MANIFEST="$OUT/files.manifest"
fi
(cd "$CORPUS" && find . -name '*.dart' -type f | sed 's#^\./##' | LC_ALL=C sort) >"$MANIFEST"
if [ -n "$MAX_FILES" ]; then
  head -n "$MAX_FILES" "$MANIFEST" >"$MANIFEST.cap" && mv "$MANIFEST.cap" "$MANIFEST"
fi
FILES=$(wc -l <"$MANIFEST" | tr -d ' ')
[ "$FILES" -gt 0 ] || die "no .dart files found under $CORPUS"
# BSD xargs skips the command on empty input; the guard above covers the
# portable side. Bytes/LOC describe exactly the measured set.
BYTES=$(tr '\n' '\0' <"$MANIFEST" | (cd "$CORPUS" && xargs -0 stat -f%z) | awk '{s+=$1} END {print s}')
LOC=$(tr '\n' '\0' <"$MANIFEST" | (cd "$CORPUS" && xargs -0 cat) | wc -l | tr -d ' ')

report_stats() {
  echo "corpus: $CORPUS"
  echo "  measured files: $FILES  loc: $LOC  bytes: $BYTES${MAX_FILES:+  (capped by --max-files $MAX_FILES)}"
  echo "  fixture: $FIXTURE (one diagnostic; gate surface)"
  echo "  caliber: tokens = ceil(code points / $CPT), uniform across legs"
}

echo "token-cost-bench: label=$LABEL"
report_stats

if [ "$DRY_RUN" -eq 1 ]; then
  sdk_desc="${DARTQ_DART_SDK:-<DARTQ_DART_SDK unset>}"
  dartq_bin="${DARTQ_BIN:-./target/release/dartq}"
  echo "token-cost-bench: DRY RUN — statistics above; a real run would execute, serially:"
  echo "  stage: cp -Rc $CORPUS/. -> $OUT/staging/ (clonefile, cp fallback)${MAX_FILES:+; selective copy of files.manifest}"
  echo "    + synthesize staging/{pubspec.yaml,.dart_tool/package_config.json} when the corpus lacks them (self rootUri ../, languageVersion 3.13)"
  echo "  corpus legs on $OUT/staging:"
  echo "    $dartq_bin analyze --format=machine --verbosity concise --sdk $sdk_desc <staging>"
  echo "    $dartq_bin analyze --format=machine --verbosity default --sdk $sdk_desc <staging>"
  echo "    $dartq_bin analyze --format=machine --verbosity verbose --sdk $sdk_desc <staging>"
  echo "    $dartq_bin analyze --format=json --sdk $sdk_desc <staging>"
  echo "    $sdk_desc/bin/dart analyze --format=machine <staging>"
  echo "  fixture legs (check face) on $FIXTURE: the same five, oracle via dart analyze --format=machine"
  echo "  gates: G1 fixture compact <= 120 tokens; G2 fixture compact saving >= 50% vs fixture oracle"
  echo "  outputs: $OUT/{versions.tsv,stats.txt,raw/,tokens.tsv,tokens-dist.tsv,summary.json,summary.md}"
  exit 0
fi

# --- real-run preflight --------------------------------------------------------
if [ -z "${DARTQ_DART_SDK:-}" ]; then
  die "DARTQ_DART_SDK is required for a real run (oracle Dart SDK root with bin/dart)"
fi
[ -d "$DARTQ_DART_SDK" ] || die "DARTQ_DART_SDK=$DARTQ_DART_SDK is not a directory"
DART="$DARTQ_DART_SDK/bin/dart"
[ -x "$DART" ] || die "dart binary not found at $DART"

DARTQ_BIN="${DARTQ_BIN:-./target/release/dartq}"
if [ ! -x "$DARTQ_BIN" ]; then
  echo "token-cost-bench: building dartq-cli (release)"
  cargo build --release -p dartq-cli --quiet
fi
[ -x "$DARTQ_BIN" ] || die "dartq binary missing after build: $DARTQ_BIN"
[ -d "$FIXTURE" ] || die "gate fixture missing: $FIXTURE (expected one main.dart with exactly one diagnostic)"

# --- stage the corpus -----------------------------------------------------------
STAGE="$OUT/staging"
RAW="$OUT/raw"
mkdir -p "$STAGE" "$RAW"
if [ -n "$MAX_FILES" ]; then
  # Selective copy: only the measured files, original relative layout.
  while IFS= read -r rel; do
    mkdir -p "$STAGE/$(dirname "$rel")"
    cp "$CORPUS/$rel" "$STAGE/$rel"
  done <"$MANIFEST"
else
  if cp -Rc "$CORPUS/." "$STAGE/" 2>/dev/null; then
    MATERIALIZE_MODE=clone
  else
    cp -R "$CORPUS/." "$STAGE/"
    MATERIALIZE_MODE=copy
  fi
fi

# The analyze face needs a package root; give the staged clone the minimal
# self-package context when the corpus does not carry one (the corpus itself
# is never touched).
[ -f "$STAGE/pubspec.yaml" ] ||
  printf 'name: token_cost_corpus\nenvironment:\n  sdk: ^3.13.0\n' >"$STAGE/pubspec.yaml"
if [ ! -f "$STAGE/.dart_tool/package_config.json" ]; then
  mkdir -p "$STAGE/.dart_tool"
  printf '{"configVersion":2,"generator":"token-cost-bench","packages":[{"name":"token_cost_corpus","rootUri":"../","languageVersion":"3.13"}]}\n' \
    >"$STAGE/.dart_tool/package_config.json"
fi

# --- environment manifest --------------------------------------------------------
DARTQ_VER=$("$DARTQ_BIN" --version 2>&1)
DART_VER=$("$DART" --version 2>&1)
GIT_REV=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
{
  printf 'key\tvalue\n'
  printf 'label\t%s\n' "$LABEL"
  printf 'utc\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'git_rev\t%s\n' "$GIT_REV"
  printf 'dartq_bin\t%s\n' "$DARTQ_BIN"
  printf 'dartq_version\t%s\n' "$DARTQ_VER"
  printf 'dart_sdk\t%s\n' "$DARTQ_DART_SDK"
  printf 'dart_version\t%s\n' "$DART_VER"
  printf 'corpus\t%s\n' "$CORPUS"
  printf 'measured_files\t%s\n' "$FILES"
  printf 'measured_loc\t%s\n' "$LOC"
  printf 'measured_bytes\t%s\n' "$BYTES"
  printf 'max_files\t%s\n' "${MAX_FILES:--}"
  printf 'chars_per_token\t%s\n' "$CPT"
  printf 'fixture\t%s\n' "$FIXTURE"
  printf 'materialization\t%s\n' "${MATERIALIZE_MODE:-selective}"
} >"$OUT/versions.tsv"

report_stats >"$OUT/stats.txt"

# --- legs -------------------------------------------------------------------
# Each leg captures full stdout (the measured payload), stderr, and the exit
# code. Nonzero exits are expected on both sides whenever diagnostics exist
# (fatal-warnings posture) — recorded, never fatal for the harness.
run_leg() { # $1 leg name; rest = command words
  local leg=$1
  shift
  set +e
  "$@" >"$RAW/$leg.txt" 2>"$RAW/$leg.err" </dev/null
  echo $? >"$RAW/$leg.exit"
  set -e
}

echo "token-cost-bench: corpus legs on $STAGE"
run_leg compact "$DARTQ_BIN" analyze --format=machine --verbosity concise --sdk "$DARTQ_DART_SDK" "$STAGE"
run_leg parity "$DARTQ_BIN" analyze --format=machine --verbosity default --sdk "$DARTQ_DART_SDK" "$STAGE"
run_leg full "$DARTQ_BIN" analyze --format=machine --verbosity verbose --sdk "$DARTQ_DART_SDK" "$STAGE"
run_leg json "$DARTQ_BIN" analyze --format=json --sdk "$DARTQ_DART_SDK" "$STAGE"
run_leg oracle "$DART" analyze --format=machine "$STAGE"

echo "token-cost-bench: fixture legs on $FIXTURE (check face)"
run_leg fixture-compact "$DARTQ_BIN" check --format=machine --verbosity concise "$FIXTURE"
run_leg fixture-parity "$DARTQ_BIN" check --format=machine --verbosity default "$FIXTURE"
run_leg fixture-full "$DARTQ_BIN" check --format=machine --verbosity verbose "$FIXTURE"
run_leg fixture-json "$DARTQ_BIN" check --format=json "$FIXTURE"
run_leg fixture-oracle "$DART" analyze --format=machine "$FIXTURE"

# --- fixture integrity -------------------------------------------------------
# The gate surface is meaningful only for exactly one diagnostic row; drift
# here is a fixture bug (infra error), not a gate verdict.
FIXTURE_ROWS=$(wc -l <"$RAW/fixture-compact.txt" | tr -d ' ')
[ "$FIXTURE_ROWS" -eq 1 ] ||
  die "fixture produced $FIXTURE_ROWS compact rows (expected exactly 1) — $FIXTURE drifted from the verified single-diagnostic shape; fix the fixture before trusting any gate"

# --- counting, gates, summaries ----------------------------------------------
status=0
python3 - "$OUT" "$CPT" <<'PY' || status=$?
import json
import math
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
cpt = int(sys.argv[2])
raw = out / "raw"
raw_bytes = {p.name: p.stat().st_size for p in raw.glob("*.txt")}

CORPUS_LEGS = ["compact", "parity", "full", "json", "oracle"]
FIXTURE_LEGS = ["fixture-compact", "fixture-parity", "fixture-full", "fixture-json", "fixture-oracle"]
JSON_LEGS = {"json", "fixture-json"}
GATE_TOKENS = 120   # the P4 release row's absolute compact ceiling
GATE_SAVING = 50.0  # the P4 release row's relative saving floor


def tokens(chars):
    return math.ceil(chars / cpt)


def measure(leg):
    """Per-leg totals + per-diagnostic token distribution.

    Totals count the whole captured payload (rows, separators, envelope);
    entries are per-diagnostic payloads: machine rows are one line each, the
    JSON envelope is split into its compact per-diagnostic objects.
    """
    text = (raw / f"{leg}.txt").read_text(encoding="utf-8")
    total_chars = len(text)
    if leg in JSON_LEGS:
        envelope = json.loads(text)
        entries = [json.dumps(d, ensure_ascii=False, separators=(",", ":"))
                   for d in envelope.get("diagnostics", [])]
    else:
        entries = [line for line in text.splitlines() if line]
    dist = [tokens(len(e)) for e in entries]
    return {
        "bytes": raw_bytes.get(f"{leg}.txt", 0),
        "chars": total_chars,
        "diags": len(entries),
        "tokens": tokens(total_chars),
        "dist": dist,
    }


def median(sorted_vals):
    n = len(sorted_vals)
    return sorted_vals[(n + 1) // 2 - 1] if n else 0


def p95(sorted_vals):
    n = len(sorted_vals)
    return sorted_vals[math.ceil(0.95 * n) - 1] if n else 0


legs = {}
for leg in CORPUS_LEGS + FIXTURE_LEGS:
    try:
        m = measure(leg)
    except (OSError, ValueError) as exc:
        # Boundary: a truncated/corrupt capture (e.g. a leg crashed mid-write)
        # is an infra error — report the leg, not a traceback.
        print(f"token-cost-bench: counting failed for {leg}: {exc} — inspect raw/{leg}.txt", file=sys.stderr)
        sys.exit(2)
    m["exit_code"] = int((raw / f"{leg}.exit").read_text().strip() or 0)
    s = sorted(m["dist"])
    m["median"] = median(s)
    m["p95"] = p95(s)
    m["tokens_per_diag"] = round(m["tokens"] / m["diags"], 2) if m["diags"] else None
    legs[leg] = m

for leg in CORPUS_LEGS + FIXTURE_LEGS:
    ref = "fixture-oracle" if leg.startswith("fixture-") else "oracle"
    oracle_tokens = legs[ref]["tokens"]
    legs[leg]["saving_pct"] = (
        round((oracle_tokens - legs[leg]["tokens"]) / oracle_tokens * 100, 1)
        if oracle_tokens > 0 else None)

with (out / "tokens.tsv").open("w", encoding="utf-8") as f:
    f.write("leg\texit_code\tbytes\tchars\tdiags\ttokens\ttokens_per_diag\tmedian\tp95\tsaving_vs_oracle_pct\n")
    for leg in CORPUS_LEGS + FIXTURE_LEGS:
        m = legs[leg]
        f.write(f"{leg}\t{m['exit_code']}\t{m['bytes']}\t{m['chars']}\t{m['diags']}\t"
                f"{m['tokens']}\t{m['tokens_per_diag']}\t{m['median']}\t{m['p95']}\t{m['saving_pct']}\n")

with (out / "tokens-dist.tsv").open("w", encoding="utf-8") as f:
    f.write("leg\tseq\tchars\ttokens\n")
    for leg in CORPUS_LEGS + FIXTURE_LEGS:
        m = legs[leg]
        f.write(f"{leg}\ttotal\t{m['chars']}\t{m['tokens']}\n")
        for i, t in enumerate(m["dist"], 1):
            f.write(f"{leg}\t{i}\t\t{t}\n")

compact = legs["fixture-compact"]
oracle = legs["fixture-oracle"]
gates = [
    {"id": "G1",
     "desc": "single-error fixture compact output <= 120 tokens",
     "value": compact["tokens"], "threshold": GATE_TOKENS,
     "pass": compact["tokens"] <= GATE_TOKENS},
    {"id": "G2",
     "desc": "single-error fixture compact saving >= 50% vs oracle machine output",
     "value": compact["saving_pct"], "threshold": GATE_SAVING,
     "pass": compact["saving_pct"] is not None and compact["saving_pct"] >= GATE_SAVING},
]

manifest = {}
for line in (out / "versions.tsv").read_text(encoding="utf-8").splitlines()[1:]:
    k, _, v = line.partition("\t")
    manifest[k] = v

summary = {
    "schema": "token-cost-summary-v1",
    "label": manifest.get("label", ""),
    "utc": manifest.get("utc", ""),
    "caliber": {
        "rule": "tokens = ceil(code points / chars_per_token)",
        "chars_per_token": cpt,
        "note": ("the benchmark charter leaves the tokenizer unpinned; deterministic "
                 "character-level approximation, uniform across every leg — saving "
                 "ratios cancel it, the absolute gate does not. Raw outputs are "
                 "captured verbatim for re-tokenization offline."),
    },
    "corpus": {"dir": manifest.get("corpus", ""), "files": int(manifest.get("measured_files", 0)),
               "loc": int(manifest.get("measured_loc", 0)), "bytes": int(manifest.get("measured_bytes", 0))},
    "legs": {leg: {k: v for k, v in legs[leg].items() if k != "dist"}
             for leg in CORPUS_LEGS + FIXTURE_LEGS},
    "gates": gates,
    "all_gates_pass": all(g["pass"] for g in gates),
}
(out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")


def fmt(v):
    return "-" if v is None else v


lines = [
    f"# Token cost benchmark — {summary['label']}", "",
    f"caliber: {summary['caliber']['rule']}, chars_per_token={cpt} "
    "(deterministic character-level approximation, uniform across legs; the "
    "benchmark charter leaves the tokenizer unpinned)", "",
    f"corpus: {summary['corpus']['dir']} (files={summary['corpus']['files']}, "
    f"loc={summary['corpus']['loc']}, bytes={summary['corpus']['bytes']})",
    f"fixture: single-error (one diagnostic; the gate surface)", "",
    "## Corpus legs (analyze face, staged clone)", "",
    "| leg | exit | diags | chars | tokens | tokens/diag | median | p95 | saving vs oracle |",
    "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
]
for leg in CORPUS_LEGS:
    m = legs[leg]
    lines.append(f"| {leg} | {m['exit_code']} | {m['diags']} | {m['chars']} | {m['tokens']} | "
                 f"{fmt(m['tokens_per_diag'])} | {m['median']} | {m['p95']} | {fmt(m['saving_pct'])}% |")
lines += ["", "## Fixture legs (check face, one diagnostic)", "",
          "| leg | exit | diags | chars | tokens | saving vs fixture oracle |",
          "| --- | --- | --- | --- | --- | --- |"]
for leg in FIXTURE_LEGS:
    m = legs[leg]
    lines.append(f"| {leg} | {m['exit_code']} | {m['diags']} | {m['chars']} | {m['tokens']} | "
                 f"{fmt(m['saving_pct'])}% |")
lines += ["", "## Gates (the P4 token-economy release row)", ""]
for g in gates:
    verdict = "PASS" if g["pass"] else "FAIL"
    lines.append(f"{g['id']}: {verdict} — {g['desc']} (value={fmt(g['value'])}, threshold={g['threshold']})")
lines += ["", f"all gates pass: {str(summary['all_gates_pass']).lower()}",
          "", "Per-diagnostic distributions: tokens-dist.tsv; re-tokenization surface: raw/."]
(out / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

for g in gates:
    verdict = "PASS" if g["pass"] else "FAIL"
    print(f"token-cost-bench: {g['id']}: {verdict} — {g['desc']} "
          f"(value={fmt(g['value'])}, threshold={g['threshold']})")
print(f"token-cost-bench: summary -> {out / 'summary.md'}")
sys.exit(0 if summary["all_gates_pass"] else 1)
PY

if [ "$status" -eq 0 ]; then
  echo "token-cost-bench: complete — all gates PASS — evidence in $OUT"
elif [ "$status" -eq 2 ]; then
  echo "token-cost-bench: counting-infrastructure failure — no gate verdict this run; partial evidence in $OUT (tokens/summary not written; inspect the counting error above)"
else
  echo "token-cost-bench: complete — gate(s) FAIL — evidence in $OUT (results recorded; the gate is the verdict, not a harness error)"
fi
exit "$status"
