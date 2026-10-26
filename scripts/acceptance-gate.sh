#!/usr/bin/env bash
# acceptance-gate.sh — the one-sided valid-syntax acceptance gate.
#
# "All syntax supported" as a measurable invariant: a corpus of purely valid
# Dart files covering every shipped feature family of the 3.15 grammar
# window (tests/corpus/acceptance/, one file per family plus a kitchen sink;
# every file modeled on the official language tests, provenance in each
# header) must yield ZERO diagnostics from the release `dartq check` face.
#
# The rule is: fail on ANY diagnostic of ANY tier — the json envelope
#   {"version":1,"diagnostics":[...]} with SEVERITY in
#   ERROR|WARNING|INFO (the machine face retired with #2606; the oracle
#   envelope is the check face's machine-shaped successor)
# Rationale: the check face is the syntax tier only (scan → parse, no
# semantic engine), and the corpus is valid by
# construction over shipped, version-enabled syntax, so a clean run produces
# zero rows of every severity — there are no known-deliberate advisory rows.
# No oracle is involved; validity is asserted by construction (see
# tests/corpus/acceptance/README.md for the family map, the pending
# experiment-gated families, and the empty needs-first-run list).
#
# Ladder position: alongside the differential gates (m0 → m1 → m2 → m3 →
# lint → sdk-matrix → mega). As a one-sided gate it can also run standalone
# at any time; it needs no fvm/oracle install and no pub get.
#
# SERIAL PHASE ONLY: the run executes the release `dartq check` binary over
# the corpus. Never run it alongside development-phase lanes (memory-incident
# discipline). --dry-run resolves every path, prints the plan, writes
# nothing, executes nothing.
#
# Usage:
#   scripts/acceptance-gate.sh --label L [--dry-run]
#   scripts/acceptance-gate.sh --dry-run        (plan only; --label optional there)
#   scripts/acceptance-gate.sh --help
#
#   --dry-run       resolve every path, print the plan and the exact
#                   command, write nothing, execute nothing.
#   --label L       REQUIRED on the run path, no default: the evidence
#                   directory is target/evidence/gate-acceptance/<label>/ and an
#                   existing directory is refused — the gate never overwrites
#                   recorded evidence. ACCEPTANCE_LABEL env is accepted too.
#
# Environment overrides: DARTQ_BIN (default ./target/release/dartq).
#
# Exit codes: 0 corpus clean (zero rows of every tier); 1 gate red (at
# least one diagnostic row — an engine defect against a valid corpus); 2
# usage or environment error (missing binary, corpus not found, label
# missing, an existing evidence directory, or a dartq tool failure — exit
# tier 4/internal or 64/usage — which is never scored as a verdict).

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

CORPUS="tests/corpus/acceptance"
OUT_ROOT="target/evidence/gate-acceptance"
DARTQ_BIN="${DARTQ_BIN:-./target/release/dartq}"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

dry_run=0
label="${ACCEPTANCE_LABEL:-}"
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --label)
    [ $# -ge 2 ] || {
      echo "acceptance-gate: --label needs a value" >&2
      exit 2
    }
    label="$2"
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "acceptance-gate: unknown argument '$1' (see scripts/acceptance-gate.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# The label names the evidence directory: required on the run path, no
# default, ever — a silent default could overwrite recorded history. The
# dry-run path stays label-free via a placeholder.
if [ -z "$label" ] && [ "$dry_run" != 1 ]; then
  echo "acceptance-gate: --label is required (no default) — pass --label L or set ACCEPTANCE_LABEL" >&2
  exit 2
fi
[ -n "$label" ] || label="<label>"

# --- preflight ---------------------------------------------------------------
# Pure-input checks run on both paths; the release-binary check is a
# run-path guard below (a dry-run must not fail — nor build — when the
# release binary is not on disk yet). No .dart_tool/package_config.json
# requirement: the check face is pub-free by design,
# unlike the harness corpora the m0 gate drives.
[ -f "$CORPUS/pubspec.yaml" ] || {
  echo "acceptance-gate: $CORPUS is not a corpus package (pubspec.yaml missing)" >&2
  exit 2
}
corpus_files="$(find "$CORPUS" -type f -name '*.dart' | sort)"
if [ -z "$corpus_files" ]; then
  echo "acceptance-gate: $CORPUS contains no .dart files" >&2
  exit 2
fi
corpus_count="$(printf '%s\n' "$corpus_files" | wc -l | tr -d ' ')"

outdir="$OUT_ROOT/$label"
if [ -e "$outdir" ]; then
  echo "acceptance-gate: $outdir already exists — refusing to overwrite recorded gate evidence (pass a new --label)" >&2
  exit 2
fi

# --- plan --------------------------------------------------------------------
echo "== acceptance-gate plan"
printf '  %-16s %s (%s files)\n' "corpus" "$CORPUS" "$corpus_count"
printf '  %-16s fail on ANY diagnostic of any tier (valid corpus, zero rows expected)\n' "rule"
printf '  %-16s %s check --format=json %s\n' "command" "$DARTQ_BIN" "$CORPUS"
printf '  %-16s %s\n' "evidence" "$outdir"
if [ "$dry_run" = 1 ]; then
  echo "== dry run: paths resolved, nothing written or executed"
  exit 0
fi

# Run-path binary guard (see preflight note above).
[ -x "$DARTQ_BIN" ] || {
  echo "acceptance-gate: $DARTQ_BIN not found — build it first: cargo build -p dartq-cli --release (or set DARTQ_BIN=<path>)" >&2
  exit 2
}

# --- run the gate -------------------------------------------------------------
# dartq check exit tiers: 0 clean/info-only, 1 fatal
# infos, 2 fatal warnings, 3 errors present, 4 internal failure, 64 usage.
# The verdict comes from the ROWS, not the exit code — info-only runs exit 0
# and still carry INFO rows this gate must fail on. Tiers 4/64 are tool
# failures, not verdicts.
mkdir -p "$outdir" || {
  echo "acceptance-gate: cannot create evidence directory $outdir" >&2
  exit 2
}
json_file="$outdir/check-output.json"

rc=0
"$DARTQ_BIN" check --format=json "$CORPUS" >"$json_file" || rc=$?
if [ "$rc" -ge 4 ]; then
  echo "acceptance-gate: dartq check failed with tool exit $rc (internal/usage tier — never a verdict); raw output:" >&2
  sed 's/^/  | /' "$json_file" >&2
  exit 2
fi
# The counters below are integer-scored; a malformed or vanished capture
# would turn every comparison into a non-integer test (a silent false
# PASS). Refuse instead — an unparsable artifact is an environment error,
# not a verdict.
[ -s "$json_file" ] || {
  echo "acceptance-gate: $json_file missing or empty after the dartq check run" >&2
  exit 2
}
# One pass over the json envelope: per-tier counts on the first line, then
# one machine-shaped evidence row per diagnostic (severity|type|code|file|
# line|col|len|message — the json fields re-spelled; legibility only, the
# verdict reads the counts).
rows_out="$(python3 - "$json_file" <<'PYEOF'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        doc = json.load(handle)
except (OSError, ValueError) as error:
    sys.exit("acceptance-gate: json output did not parse (%s)" % error)
if doc.get("version") != 1:
    sys.exit("acceptance-gate: unexpected envelope version %r" % doc.get("version"))
by_sev = {"ERROR": 0, "WARNING": 0, "INFO": 0}
rows = []
for diag in doc.get("diagnostics", []):
    sev = diag.get("severity")
    if sev in by_sev:
        by_sev[sev] += 1
    location = diag.get("location", {})
    rng = location.get("range", {})
    start = rng.get("start", {})
    rows.append("%s|%s|%s|%s|%s|%s|%s|%s" % (
        sev, diag.get("type", "?"), diag.get("code", "?"),
        location.get("file", "?"), start.get("line", "?"),
        start.get("column", "?"),
        rng.get("end", {}).get("offset", 0) - start.get("offset", 0),
        diag.get("problemMessage", "?"),
    ))
print("%(ERROR)s %(WARNING)s %(INFO)s" % by_sev)
for row in rows:
    print(row)
PYEOF
)" || exit 2
error_rows="$(printf '%s\n' "$rows_out" | sed -n 1p | cut -d' ' -f1)"
warning_rows="$(printf '%s\n' "$rows_out" | sed -n 1p | cut -d' ' -f2)"
info_rows="$(printf '%s\n' "$rows_out" | sed -n 1p | cut -d' ' -f3)"
rows=$((error_rows + warning_rows + info_rows))

if [ "$rows" -gt 0 ]; then
  echo "-- [$label] diagnostics over the valid corpus ($rows rows: $error_rows error, $warning_rows warning, $info_rows info):"
  printf '%s\n' "$rows_out" | sed '1d' | sed 's/^/  | /'
  verdict=FAIL
else
  echo "-- [$label] corpus clean: zero rows of every tier"
  verdict=PASS
fi

# --- evidence + summary --------------------------------------------------------
{
  echo "gate: scripts/acceptance-gate.sh"
  echo "corpus: $CORPUS ($corpus_count .dart files)"
  echo "rule: fail on ANY diagnostic of any tier (valid corpus, zero rows expected)"
  echo "command: $DARTQ_BIN check --format=json $CORPUS"
  echo "dartq check exit: $rc"
  echo "rows: $rows (error=$error_rows warning=$warning_rows info=$info_rows)"
  echo "verdict: $verdict"
} >"$outdir/verdict.txt"

printf '  %-16s %s (error=%s warning=%s info=%s)\n' "rows" "$rows" "$error_rows" "$warning_rows" "$info_rows"
printf '  %-16s %s\n' "evidence" "$outdir/check-output.json , $outdir/verdict.txt"

if [ "$verdict" = "PASS" ]; then
  echo "acceptance-gate: VERDICT PASS (valid-syntax corpus yields zero diagnostics of every tier)"
  exit 0
fi
echo "acceptance-gate: VERDICT FAIL (diagnostics over a valid corpus — see the rows above; evidence: $outdir)"
exit 1
