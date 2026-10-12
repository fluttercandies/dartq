#!/usr/bin/env bash
# acceptance-gate.sh — the one-sided valid-syntax acceptance gate.
#
# "All syntax supported" as a measurable invariant: a corpus of purely valid
# Dart files covering every shipped feature family of the 3.15 grammar
# window (tests/corpus/acceptance/, one file per family plus a kitchen sink;
# every file modeled on the official language tests, provenance in each
# header) must yield ZERO diagnostics from the release `dartq check` face.
#
# The rule is: fail on ANY machine row of ANY tier —
#   SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE  (SEVERITY in
#   ERROR|WARNING|INFO)
# Rationale: the check face is the syntax tier only (scan → parse, no
# semantic engine; engine-api-spec §2.1), and the corpus is valid by
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
#                   directory is docs/audits/gate-acceptance/<label>/ and an
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
OUT_ROOT="docs/audits/gate-acceptance"
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
# requirement: the check face is pub-free by design (engine-api-spec §2.1),
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
printf '  %-16s fail on ANY machine row of any tier (valid corpus, zero rows expected)\n' "rule"
printf '  %-16s %s check --format=machine %s\n' "command" "$DARTQ_BIN" "$CORPUS"
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
# dartq check exit tiers (engine-api-spec §4.2): 0 clean/info-only, 1 fatal
# infos, 2 fatal warnings, 3 errors present, 4 internal failure, 64 usage.
# The verdict comes from the ROWS, not the exit code — info-only runs exit 0
# and still carry INFO rows this gate must fail on. Tiers 4/64 are tool
# failures, not verdicts.
mkdir -p "$outdir"
machine_file="$outdir/machine-output.txt"

rc=0
"$DARTQ_BIN" check --format=machine "$CORPUS" >"$machine_file" || rc=$?
if [ "$rc" -ge 4 ]; then
  echo "acceptance-gate: dartq check failed with tool exit $rc (internal/usage tier — never a verdict); raw output:" >&2
  sed 's/^/  | /' "$machine_file" >&2
  exit 2
fi

rows="$(grep -Ec '^(ERROR|WARNING|INFO)\|' "$machine_file" || true)"
error_rows="$(grep -Ec '^ERROR\|' "$machine_file" || true)"
warning_rows="$(grep -Ec '^WARNING\|' "$machine_file" || true)"
info_rows="$(grep -Ec '^INFO\|' "$machine_file" || true)"

if [ "$rows" -gt 0 ]; then
  echo "-- [$label] diagnostics over the valid corpus ($rows rows: $error_rows error, $warning_rows warning, $info info):"
  grep -E '^(ERROR|WARNING|INFO)\|' "$machine_file" | sed 's/^/  | /'
  verdict=FAIL
else
  echo "-- [$label] corpus clean: zero rows of every tier"
  verdict=PASS
fi

# --- evidence + summary --------------------------------------------------------
{
  echo "gate: scripts/acceptance-gate.sh"
  echo "corpus: $CORPUS ($corpus_count .dart files)"
  echo "rule: fail on ANY machine row of any tier (valid corpus, zero rows expected)"
  echo "command: $DARTQ_BIN check --format=machine $CORPUS"
  echo "dartq check exit: $rc"
  echo "rows: $rows (error=$error_rows warning=$warning_rows info=$info_rows)"
  echo "verdict: $verdict"
} >"$outdir/verdict.txt"

printf '  %-16s %s (error=%s warning=%s info=%s)\n' "rows" "$rows" "$error_rows" "$warning_rows" "$info_rows"
printf '  %-16s %s\n' "evidence" "$outdir/machine-output.txt , $outdir/verdict.txt"

if [ "$verdict" = "PASS" ]; then
  echo "acceptance-gate: VERDICT PASS (valid-syntax corpus yields zero diagnostics of every tier)"
  exit 0
fi
echo "acceptance-gate: VERDICT FAIL (diagnostics over a valid corpus — see the rows above; evidence: $outdir)"
exit 1
