# common.sh — the shared harness of the dartq process-level e2e suite.
#
# One leg = one externally observable contract of the release binary over
# a real (generated or tracked) package tree: stdout/stderr/exit code are
# captured under target/evidence/e2e/, verdicts land in legs.tsv, and the
# runner exits non-zero when any leg fails. Every helper asserts through
# `bad`/`ok` so a leg keeps collecting evidence after a failed check.
#
# Isolation: the suite runs with XDG_CACHE_HOME pointed at a scratch dir,
# so the session registry, the L3 content store and the test-result cache
# never touch (or read) the invoking user's caches. All generated trees
# live under one scratch root that is removed at exit, and the cleanup
# trap stops every session the suite started (the zero-orphan rule,
# #2583: no dartq/dart/flutter process outlives the suite).

# The suite root (this file lives at tests/e2e/lib/common.sh) and the
# repo root two levels above it.
E2E_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$E2E_ROOT/../.." && pwd)"
DARTQ="${DARTQ:-$REPO_ROOT/target/release/dartq}"
EVIDENCE="$REPO_ROOT/target/evidence/e2e"

# The real Dart SDK (the fvm default, like scripts/cli-parity-matrix.sh).
# Only the dart-toolchain legs (real `dart pub get`, `dartq test` faces)
# need it; everything else runs hermetically over generated stub SDKs.
DARTQ_DART_SDK="${DARTQ_DART_SDK:-$HOME/fvm/default/bin/cache/dart-sdk}"

# Scratch root: always under /tmp (unix sockets cap at ~104 path bytes —
# the macOS per-user TMPDIR is too deep), created once per suite run,
# removed by the cleanup trap. Resolved to its physical spelling (macOS
# /tmp -> /private/tmp) so needle paths match the engine's canonicalized
# output.
E2E_WORK="$(mktemp -d /tmp/dqe.XXXXXX)"
E2E_WORK="$(cd "$E2E_WORK" && pwd -P)"
export XDG_CACHE_HOME="$E2E_WORK/cache"
mkdir -p "$XDG_CACHE_HOME"

FAILS=()
SKIPS=()
CURRENT_LEG="-"

# ----------------------------------------------------------------------
# Verdict plumbing
# ----------------------------------------------------------------------

# Starts a leg: every ok/bad until the next leg_begin belongs to it.
leg_begin() {
  CURRENT_LEG="$1"
  printf '%s\n' "=== $CURRENT_LEG: $2"
}

ok() { printf '  ok    %s\n' "$1"; }

# Records a failure with evidence pointers and keeps going.
bad() {
  FAILS+=("$CURRENT_LEG: $1")
  printf '  FAIL  %s\n' "$1"
  if [ -n "${2:-}" ]; then
    printf '        %s\n' "$2"
  fi
}

# Records a skip (an environmental prerequisite is missing) and keeps
# going; the summary reports skips separately from failures.
skip() {
  SKIPS+=("$CURRENT_LEG: $1")
  printf '  SKIP  %s\n' "$1"
}

assert_eq() { # actual expected description
  if [ "$1" = "$2" ]; then ok "$3"; else
    bad "$3" "expected [$2], got [$1]"
  fi
}

assert_contains() { # haystack needle description
  case "$1" in
    *"$2"*) ok "$3" ;;
    *) bad "$3" "missing [$2] in: $(printf '%s' "$1" | head -c 400)" ;;
  esac
}

assert_not_contains() { # haystack needle description
  case "$1" in
    *"$2"*) bad "$3" "unexpected [$2] in: $(printf '%s' "$1" | head -c 400)" ;;
    *) ok "$3" ;;
  esac
}

# json_check FILE EXPR DESCRIPTION — EXPR is a python expression over the
# parsed JSON document `j` (single JSON object or one JSONL line at a
# time via jsonl_check).
json_check() {
  local file="$1" expr="$2" desc="$3"
  if python3 -c "
import json, sys
try:
    j = json.load(open('$file'))
except Exception as error:
    sys.exit(f'unparsable JSON: {error}')
sys.exit(0 if ($expr) else 1)
"; then ok "$desc"; else
    bad "$desc" "python expr failed: $expr (in $file)"
  fi
}

# jsonl_check FILE EXPR DESCRIPTION — EXPR is evaluated per line over
# `j`; every line must satisfy it and the file must be non-empty.
jsonl_check() {
  local file="$1" expr="$2" desc="$3"
  if python3 -c "
import json, sys
lines = [l for l in open('$file').read().splitlines() if l.strip()]
if not lines:
    sys.exit('no JSONL lines')
for line in lines:
    j = json.loads(line)
    if not ($expr):
        sys.exit(f'line failed: {line[:200]}')
"; then ok "$desc"; else
    bad "$desc" "$(python3 -c "
import json, sys
lines = [l for l in open('$file').read().splitlines() if l.strip()]
if not lines:
    sys.exit('no JSONL lines')
for line in lines:
    j = json.loads(line)
    if not ($expr):
        sys.exit(f'line failed: {line[:200]}')
" 2>&1 | head -1)"
  fi
}

# ----------------------------------------------------------------------
# Process running
# ----------------------------------------------------------------------

# jsonl_count FILE [TYPE] — prints how many JSONL rows carry `type` ==
# TYPE (all rows when TYPE is absent); prints -1 on a parse error.
jsonl_count() {
  python3 - "$1" "${2:-}" <<'PYEOF'
import json, sys
count = 0
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    row = json.loads(line)
    if len(sys.argv) < 3 or sys.argv[2] == "" or row.get("type") == sys.argv[2]:
        count += 1
print(count)
PYEOF
}

# jsonl_field FILE TYPE FIELD — prints the field of the FIRST row of the
# given type (empty when absent).
jsonl_field() {
  python3 - "$1" "$2" "$3" <<'PYEOF'
import json, sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    row = json.loads(line)
    if row.get("type") == sys.argv[2]:
        print(row.get(sys.argv[3], ""))
        break
PYEOF
}

# assert_fields FILE ROWPREFIX COUNT DESCRIPTION — the first machine row
# starting with ROWPREFIX must split into exactly COUNT `|`-fields.
assert_fields() {
  local file="$1" prefix="$2" count="$3" desc="$4"
  if python3 - "$file" "$prefix" "$count" <<'PYEOF'
import sys
file, prefix, count = sys.argv[1], sys.argv[2], int(sys.argv[3])
rows = [l.rstrip("\n") for l in open(file) if l.startswith(prefix)]
assert rows, f"no {prefix} row in {file}"
fields = rows[0].split("|")
assert len(fields) == count, f"{len(fields)} fields (want {count}): {rows[0]}"
PYEOF
  then ok "$desc"; else bad "$desc" "field count mismatch in $file"; fi
}

# run_dartq ID ARGS... — runs the release binary, captures
# $EVIDENCE/$ID.{out,err,rc}, echoes stdout, sets DQ_RC.
run_dartq() {
  local id="$1"; shift
  local rc=0
  "$DARTQ" "$@" >"$EVIDENCE/$id.out" 2>"$EVIDENCE/$id.err" || rc=$?
  echo "$rc" >"$EVIDENCE/$id.rc"
  DQ_RC=$rc
  cat "$EVIDENCE/$id.out"
}

# run_dartq_in ID DIR ARGS... — run_dartq with a working directory.
run_dartq_in() {
  local id="$1" dir="$2"; shift 2
  local rc=0
  ( cd "$dir" && "$DARTQ" "$@" ) >"$EVIDENCE/$id.out" 2>"$EVIDENCE/$id.err" || rc=$?
  echo "$rc" >"$EVIDENCE/$id.rc"
  DQ_RC=$rc
  cat "$EVIDENCE/$id.out"
}

err_of() { cat "$EVIDENCE/$1.err"; }
out_file() { printf '%s\n' "$EVIDENCE/$1.out"; }

# ----------------------------------------------------------------------
# Fixture builders (generated where large, tracked where small)
# ----------------------------------------------------------------------

# write FILE CONTENT — mkdir -p the parent first.
wf() {
  mkdir -p "$(dirname "$1")"
  printf '%s' "$2" >"$1"
}

# make_pkg DIR NAME LANGUAGE_VERSION — a minimal resolvable package:
# pubspec.yaml + .dart_tool/package_config.json (self only) + lib/.
make_pkg() {
  local dir="$1" name="$2" lang="${3:-3.5}"
  wf "$dir/pubspec.yaml" "name: $name
environment:
  sdk: ^3.5.0
"
  wf "$dir/.dart_tool/package_config.json" "{
  \"configVersion\": 2,
  \"packages\": [
    {\"name\": \"$name\", \"rootUri\": \"../\", \"packageUri\": \"lib/\", \"languageVersion\": \"$lang\"}
  ],
  \"generator\": \"dartq-e2e\"
}"
  mkdir -p "$dir/lib"
}

# make_stub_sdk DIR [VERSION] — the minimal SDK-shaped root the engine
# assembles against (version, libraries.json, allowed_experiments.json,
# an empty dart:core): the same shape the daemon/session unit fixtures
# use, so the whole engine tier runs hermetically.
make_stub_sdk() {
  local dir="$1" version="${2:-3.13.1}"
  wf "$dir/version" "$version
"
  wf "$dir/lib/libraries.json" '{
  "vm": {"libraries": {"core": {"uri": "core/core.dart"}}}
}'
  wf "$dir/lib/_internal/allowed_experiments.json" '{
  "version": 1,
  "experimentSets": {"sdkExperiments": []},
  "sdk": {"default": {"experimentSet": "sdkExperiments"}},
  "packages": {}
}'
  wf "$dir/lib/core/core.dart" 'library dart.core;
'
}

# gen_files TREE COUNT [PREFIX] — COUNT tiny clean libraries under
# lib/src/, the cheap way to make cold analysis observable in time.
gen_files() {
  local tree="$1" count="$2" prefix="${3:-gen}"
  mkdir -p "$tree/lib/src"
  python3 - "$tree" "$count" "$prefix" <<'PYEOF'
import sys
tree, count, prefix = sys.argv[1], int(sys.argv[2]), sys.argv[3]
for i in range(count):
    with open(f"{tree}/lib/src/{prefix}_{i:04d}.dart", "w") as f:
        f.write(f"int {prefix}_{i:04d}() => {i};\n")
PYEOF
}

# ----------------------------------------------------------------------
# Session helpers
# ----------------------------------------------------------------------

# session_pid TREE — the serve pid from the worktree's state.json.
session_pid() {
  python3 -c "
import json
print(json.load(open('$1/.dart_tool/dartq/state.json'))['pid'])
"
}

# ----------------------------------------------------------------------
# Cleanup (the zero-orphan rule)
# ----------------------------------------------------------------------

e2e_fini() {
  local rc=$?
  # Stop every session this suite started: the isolated XDG_CACHE_HOME
  # scopes `stop --all` to the suite's own registry.
  "$DARTQ" session stop --all >/dev/null 2>&1 || true
  # Belt: any serve process still holding a scratch tree dies here.
  pkill -9 -f "dartq session serve --dir $E2E_WORK" 2>/dev/null || true
  chmod -R u+w "$E2E_WORK" 2>/dev/null || true
  rm -rf "$E2E_WORK"
  # bash 3.2 (macOS): guard empty arrays against set -u.
  if [ "$rc" -eq 0 ] && [ "${FAILS[@]+x}" ] && [ "${#FAILS[@]}" -gt 0 ]; then rc=1; fi
  printf '\n'
  if [ "${SKIPS[@]+x}" ] && [ "${#SKIPS[@]}" -gt 0 ]; then
    printf 'skipped legs (%d):\n' "${#SKIPS[@]}"
    printf '  %s\n' "${SKIPS[@]}"
  fi
  if [ "${FAILS[@]+x}" ] && [ "${#FAILS[@]}" -gt 0 ]; then
    printf 'failed legs (%d):\n' "${#FAILS[@]}"
    printf '  %s\n' "${FAILS[@]}"
    exit 1
  fi
  printf 'all legs passed\n'
  exit "$rc"
}
trap e2e_fini EXIT
