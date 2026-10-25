#!/usr/bin/env bash
# test-face-parity.sh — end-to-end parity of the `dartq test` face against
# the stock runner it replaces (`dart test` / `flutter test`) on a small
# fixture package.
#
# The unit suites (`cargo test -p dartq-test --lib`, the CLI test module)
# pin the face's machinery offline; this lane pins its BEHAVIOR against the
# real toolchain the caller installed. The test-face-matrix workflow runs
# it across a band of Dart and Flutter stable versions: the flutter path
# mirrors flutter_tools internals through the verbose-trace probe
# (Recipe::probe), which can drift between versions with nothing in-repo
# noticing — this script is that notice.
#
# Legs (per mode, over the fixture's suites):
#   pass   one green suite — oracle vs dartq cold vs dartq warm; the warm
#          run must REPLAY the cold run from the result cache, and its
#          rows must still match the oracle's
#   fail   one red suite — rows and the nonzero exit code must match
#   all    the whole test/ directory (several suites share one grouped
#          program / one compiled chunk) — cold and warm, rows and code
#
# The compared face is the (outcome, suite, name) row set extracted from
# the runner's JSON event stream (`-r json` / `--machine`): one row per
# non-hidden leaf test, order-free. Everything else — times, ids, print
# noise, the flutter tool's device events — is deliberately outside it.
#
# The warm legs also read dartq's own `--stats` line and require at least
# one replayed suite: a warm leg that silently re-ran everything would
# pass row parity while exercising no cache replay at all. If that line's
# wording drifts, this guard fails loudly and the regex below is updated —
# the coupling is deliberate, not accidental.
#
# Usage:
#   scripts/test-face-parity.sh --dart    tests/testface/dart_pkg
#   scripts/test-face-parity.sh --flutter tests/testface/flutter_pkg
#
# The fixture must already be pub-getted; `dart` (or `flutter`) must be on
# PATH, as must ./target/release/dartq (cargo build -p dartq-cli
# --release). DARTQ_CACHE_DIR is pointed at a fresh temp dir, so cold legs
# are truly cold and warm legs share exactly the cold run's store.
# Evidence (per-leg event streams, row sets, exit codes, tool versions)
# lands under target/evidence/test-face/<mode>/.
# Exit codes: 0 parity holds; 1 a parity leg diverged; 2 environment.

set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

REPO_ROOT="$PWD"
DARTQ="$REPO_ROOT/target/release/dartq"
OUT_ROOT="$REPO_ROOT/target/evidence/test-face"

usage() {
  sed -n '2,/^set -u -o pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() { echo "test-face-parity: $1" >&2; exit 2; }

mode=""
fixture=""
while [ $# -gt 0 ]; do
  case "$1" in
  --dart | --flutter)
    [ -z "$mode" ] || die "one mode only (--dart or --flutter)"
    [ $# -ge 2 ] || die "$1 needs a fixture package path"
    mode="${1#--}"
    fixture="$2"
    shift 2
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/test-face-parity.sh --help)"
    ;;
  esac
done
[ -n "$mode" ] || die "pick a mode: --dart or --flutter (see --help)"
case "$fixture" in
/*) ;; # absolute path, taken as is
*) fixture="$REPO_ROOT/$fixture" ;;
esac
[ -d "$fixture/test" ] || die "fixture package has no test/ directory: $fixture"
[ -f "$fixture/pubspec.yaml" ] || die "fixture package has no pubspec.yaml: $fixture"
[ -x "$DARTQ" ] || die "dartq binary missing at $DARTQ (cargo build -p dartq-cli --release)"
command -v python3 >/dev/null 2>&1 || die "python3 is required"

# The oracle and dartq invocations per mode. `--stats` rides along on
# every dartq leg so the warm-replay guard can read it and so the cache
# keying never differs between the cold and warm legs of one lane.
case "$mode" in
dart)
  command -v dart >/dev/null 2>&1 || die "dart is not on PATH"
  ORACLE=(dart test -r json)
  DARTQ_LEG=("$DARTQ" test -r json --stats)
  PASS_SUITE=test/pass_test.dart
  FAIL_SUITE=test/fail_test.dart
  ;;
flutter)
  command -v flutter >/dev/null 2>&1 || die "flutter is not on PATH"
  ORACLE=(flutter test --machine)
  DARTQ_LEG=("$DARTQ" test --flutter --machine --stats)
  PASS_SUITE=test/widget_test.dart
  FAIL_SUITE=test/fail_widget_test.dart
  ;;
esac

out="$OUT_ROOT/$mode"
rm -rf "$out"
mkdir -p "$out" || exit 2
cache="$(mktemp -d "${TMPDIR:-/tmp}/dartq-testface.XXXXXX")" || exit 2
export DARTQ_CACHE_DIR="$cache"

# Record the toolchain of record into the evidence dir.
if [ "$mode" = dart ]; then
  dart --version >"$out/toolchain.txt" 2>&1
else
  flutter --version >"$out/toolchain.txt" 2>&1
fi
echo "dartq: $DARTQ" >>"$out/toolchain.txt"

# run_leg <name> <cmd...> — one leg's run inside the fixture; stdout, stderr
# and the exit code land in <out>/<name>.{out,err,rc}.
run_leg() { # <name> <cmd...>
  local name="$1"
  shift
  (cd "$fixture" && "$@") >"$out/$name.out" 2>"$out/$name.err"
  printf '%s\n' "$?" >"$out/$name.rc"
}

# rows <events-file> — the sorted (outcome, suite, name) row set of one
# event stream: suite paths keyed to basenames (the oracle prints absolute
# paths), leaf tests keyed by their testDone (hidden entries are not rows;
# neither are the "loading <suite>" pseudo-tests — flutter's machine stream
# reports them unhidden, they are runner machinery, and a suite that fails
# to load still diverges through its missing test rows and exit code),
# skipped folded into the outcome.
rows() { # <events-file>
  python3 - "$1" <<'PY'
import json, os, sys

suites, tests, rows = {}, {}, []
try:
    with open(sys.argv[1], encoding="utf-8", errors="replace") as stream:
        for line in stream:
            line = line.strip()
            if not line.startswith("{"):
                continue  # raw test output / tool noise, not protocol events
            event = json.loads(line)
            kind = event.get("type")
            if kind == "suite":
                suite = event["suite"]
                suites[suite["id"]] = os.path.basename(suite.get("path") or "?")
            elif kind == "testStart":
                test = event["test"]
                tests[test["id"]] = (test.get("name", "?"), test.get("suiteID"))
            elif kind == "testDone" and not event.get("hidden"):
                name, suite_id = tests.get(event.get("testID"), ("?", None))
                if not name.startswith("loading "):
                    outcome = "skipped" if event.get("skipped") else event.get("result", "?")
                    rows.append("%s\t%s\t%s" % (outcome, suites.get(suite_id, "?"), name))
except (OSError, ValueError, KeyError, TypeError) as error:
    sys.exit("test-face-parity: malformed event stream %s: %s" % (sys.argv[1], error))
for row in sorted(rows):
    print(row)
PY
}

FAILS=()

row_fail() { # <leg> <oracle-rows> <dartq-rows>
  echo "test-face-parity: FAIL $1 — the (outcome, suite, name) row sets diverge (- oracle, + dartq):" >&2
  diff -u "$2" "$3" >&2 || true
  FAILS+=("$1(rows)")
}

rc_fail() { # <leg> <detail>
  echo "test-face-parity: FAIL $1 — exit-code mismatch: $2" >&2
  FAILS+=("$1(rc)")
}

# leg_rows <leg> <oracle-events> <dartq-events> — extract both row sets and
# compare them.
leg_rows() { # <leg> <oracle-events> <dartq-events>
  local leg="$1"
  rows "$2" >"$2.rows" || die "row extraction failed for $2"
  rows "$3" >"$3.rows" || die "row extraction failed for $3"
  diff -q "$2.rows" "$3.rows" >/dev/null 2>&1 || row_fail "$leg" "$2.rows" "$3.rows"
}

# replayed <stats-stderr> — the replayed-suite count from dartq's --stats
# line, or nothing when the line is absent.
replayed() { # <stats-stderr>
  sed -n 's/.* \([0-9][0-9]*\) replayed,.*/\1/p' "$1" | head -n1
}

rc_of() { printf '%s\n' "$(cat "$1")"; }

# -- leg: pass (green suite, cold + warm) ---------------------------------

run_leg pass-oracle "${ORACLE[@]}" "$PASS_SUITE"
run_leg pass-cold "${DARTQ_LEG[@]}" "$PASS_SUITE"
run_leg pass-warm "${DARTQ_LEG[@]}" "$PASS_SUITE"

leg_rows pass "$out/pass-oracle.out" "$out/pass-cold.out"
leg_rows pass-warm "$out/pass-oracle.out" "$out/pass-warm.out"

oracle_rc="$(rc_of "$out/pass-oracle.rc")"
cold_rc="$(rc_of "$out/pass-cold.rc")"
warm_rc="$(rc_of "$out/pass-warm.rc")"
if [ "$oracle_rc" != 0 ] || [ "$cold_rc" != 0 ] || [ "$warm_rc" != 0 ]; then
  rc_fail pass "oracle=$oracle_rc dartq-cold=$cold_rc dartq-warm=$warm_rc (a green leg must be 0)"
fi

count="$(replayed "$out/pass-warm.err")"
if [ -z "$count" ] || [ "$count" -lt 1 ]; then
  echo "test-face-parity: FAIL pass-warm — the warm run replayed no suite (--stats: $(tail -n1 "$out/pass-warm.err"))" >&2
  FAILS+=("pass-warm(replay)")
fi

# -- leg: fail (red suite) -------------------------------------------------

run_leg fail-oracle "${ORACLE[@]}" "$FAIL_SUITE"
run_leg fail-cold "${DARTQ_LEG[@]}" "$FAIL_SUITE"

leg_rows fail "$out/fail-oracle.out" "$out/fail-cold.out"

oracle_rc="$(rc_of "$out/fail-oracle.rc")"
cold_rc="$(rc_of "$out/fail-cold.rc")"
if [ "$oracle_rc" != "$cold_rc" ] || [ "$cold_rc" = 0 ]; then
  rc_fail fail "oracle=$oracle_rc dartq=$cold_rc (a red leg must be equal and nonzero)"
fi

# -- leg: all (the whole test/ directory, cold + warm) ----------------------

run_leg all-oracle "${ORACLE[@]}"
run_leg all-cold "${DARTQ_LEG[@]}"
run_leg all-warm "${DARTQ_LEG[@]}"

leg_rows all "$out/all-oracle.out" "$out/all-cold.out"
leg_rows all-warm "$out/all-oracle.out" "$out/all-warm.out"

oracle_rc="$(rc_of "$out/all-oracle.rc")"
cold_rc="$(rc_of "$out/all-cold.rc")"
warm_rc="$(rc_of "$out/all-warm.rc")"
if [ "$oracle_rc" != "$cold_rc" ] || [ "$cold_rc" != "$warm_rc" ] || [ "$cold_rc" = 0 ]; then
  rc_fail all "oracle=$oracle_rc dartq-cold=$cold_rc dartq-warm=$warm_rc (the mixed leg must be equal and nonzero)"
fi

# -- verdict -----------------------------------------------------------------

rm -rf "$cache"
if [ "${#FAILS[@]}" -eq 0 ]; then
  echo "test-face-parity: PASS ($mode face over $fixture; evidence under $out)"
  exit 0
fi
echo "test-face-parity: FAIL — divergent legs: ${FAILS[*]}" >&2
echo "  evidence: $out (per-leg event streams, row sets, exit codes, toolchain.txt)" >&2
exit 1
