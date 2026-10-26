# legs/08-cross.sh — cross-verb situations: a session alive while `dart
# pub get` changes the SDK resolution (the next command reacts, no stale
# universe); a file edited between two commands (the second command sees
# the delta — no stale diagnostics, one-shot and resident); two
# sequential CLI users on one worktree (concurrent one-shots serialize
# sanely: both succeed, byte-identical output).

legs_08_run() {
  local base="$E2E_WORK/cross"
  mkdir -p "$base"

  # --- 08.1 an edit between two commands -------------------------------
  leg_begin cross-edit-delta "an edit between two commands: the second sees the delta"
  make_stub_sdk "$base/sdk"
  make_pkg "$base/ed" "ed"
  wf "$base/ed/lib/main.dart" 'int main() => 1;
'
  # One-shot: clean → inject an error → the error appears; fix the file →
  # clean again (both directions, no stale carry-over).
  run_dartq_in ed-clean "$base/ed" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 0 "the clean tree analyzes clean"
  wf "$base/ed/lib/main.dart" 'int main() => 1 oops;
'
  run_dartq_in ed-dirty "$base/ed" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 3 "the injected error flips the exit tier"
  assert_contains "$(cat "$(out_file ed-dirty)")" "expected_token" \
    "the second command reports the new diagnostic"
  wf "$base/ed/lib/main.dart" 'int main() => 1;
'
  run_dartq_in ed-clean2 "$base/ed" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 0 "the reverted file is clean again (no stale rows)"

  # Resident: the same delta through the session's engine (MCP analyze).
  run_dartq_in ed-start "$base/ed" session start "$base/ed" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the resident session starts"
  cat >"$EVIDENCE/ed-mcp-a.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {}},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false"]}
]}
SCRIPT
  cat >"$EVIDENCE/ed-mcp-b.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {}},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false",
              "result.content.0.text contains=expected_token"]}
]}
SCRIPT
  cat >"$EVIDENCE/ed-mcp-c.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {}},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/ed" "$base/sdk" \
    "$EVIDENCE/ed-mcp-a.json" >"$EVIDENCE/ed-mcp-a.log" 2>&1
  assert_eq "$?" 0 "resident analyze over the clean tree"
  wf "$base/ed/lib/main.dart" 'int main() => 1 oops;
'
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/ed" "$base/sdk" \
    "$EVIDENCE/ed-mcp-b.json" >"$EVIDENCE/ed-mcp-b.log" 2>&1
  assert_eq "$?" 0 "resident analyze sees the injected error (no stale clean view)"
  wf "$base/ed/lib/main.dart" 'int main() => 1;
'
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/ed" "$base/sdk" \
    "$EVIDENCE/ed-mcp-c.json" >"$EVIDENCE/ed-mcp-c.log" 2>&1
  assert_eq "$?" 0 "resident analyze drops the fixed diagnostic (no stale error rows)"
  run_dartq_in ed-stop "$base/ed" session stop "$base/ed"
  assert_eq "$DQ_RC" 0 "resident teardown"

  # --- 08.2 two sequential CLI users on one worktree --------------------
  leg_begin cross-two-users "two concurrent CLI users on one worktree serialize sanely"
  make_pkg "$base/cc" "cc"
  wf "$base/cc/lib/main.dart" "import 'alpha.dart';

int main() => alpha();
"
  wf "$base/cc/lib/alpha.dart" 'int alpha() => 1;
'
  local a_rc=0 b_rc=0
  ( cd "$base/cc" && "$DARTQ" analyze --sdk-path "$base/sdk" lib >"$EVIDENCE/cc-a.out" 2>"$EVIDENCE/cc-a.err" ) || a_rc=$?
  ( cd "$base/cc" && "$DARTQ" analyze --sdk-path "$base/sdk" lib >"$EVIDENCE/cc-b.out" 2>"$EVIDENCE/cc-b.err" ) &
  local bg=$!
  ( cd "$base/cc" && "$DARTQ" analyze --sdk-path "$base/sdk" lib >"$EVIDENCE/cc-a2.out" 2>"$EVIDENCE/cc-a2.err" ) || a_rc=$?
  wait "$bg" || b_rc=$?
  assert_eq "$a_rc" 0 "the foreground user succeeds"
  assert_eq "$b_rc" 0 "the background user succeeds"
  if cmp -s "$EVIDENCE/cc-b.out" "$EVIDENCE/cc-a2.out"; then
    ok "both concurrent answers are byte-identical (deterministic, no interleaving damage)"
  else
    bad "both concurrent answers are byte-identical" \
      "differs: $(head -c 200 "$EVIDENCE/cc-b.out") vs $(head -c 200 "$EVIDENCE/cc-a2.out")"
  fi

  # --- 08.3 session alive across a pub get (needs a real Dart SDK) -------
  # The delta: pg/lib/main.dart imports package:dep/extra.dart from the
  # start. Before the dependency resolves, analyze reports the dangling
  # import; after a real `dart pub get` brings the path dependency in,
  # the same command analyzes clean — and the session that was alive
  # across both pub gets answers over the changed universe too.
  leg_begin cross-pub-get "session alive while pub get changes the SDK resolution"
  if [ ! -x "$DARTQ_DART_SDK/bin/dart" ]; then
    skip "real Dart SDK not found at $DARTQ_DART_SDK (set DARTQ_DART_SDK)"
    return 0
  fi
  make_pkg "$base/pg" "pg"
  wf "$base/pg/lib/main.dart" "import 'package:dep/extra.dart';

int main() => extra();
"
  run_dartq_in pg-start "$base/pg" session start "$base/pg" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the session starts on the real SDK"
  local pg_pid
  pg_pid="$(session_pid "$base/pg")"
  # Before: the resolution does not know dep — the import dangles.
  ( cd "$base/pg" && "$DARTQ_DART_SDK/bin/dart" pub get ) >"$EVIDENCE/pg-get1.log" 2>&1
  assert_eq "$?" 0 "the first pub get succeeds"
  run_dartq_in pg-before "$base/pg" analyze --sdk-path "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 3 "the dangling import is an error before the dependency exists"
  # The dependency joins the pubspec; a real pub get resolves it.
  make_pkg "$base/dep" "dep"
  wf "$base/dep/lib/extra.dart" 'int extra() => 1;
'
  wf "$base/pg/pubspec.yaml" 'name: pg
environment:
  sdk: ^3.5.0
dependencies:
  dep:
    path: ../dep
'
  ( cd "$base/pg" && "$DARTQ_DART_SDK/bin/dart" pub get ) >"$EVIDENCE/pg-get2.log" 2>&1
  assert_eq "$?" 0 "the second pub get succeeds (the path dep resolves)"
  if kill -0 "$pg_pid" 2>/dev/null; then
    ok "the resident session survived both pub gets"
  else
    bad "the resident session survived both pub gets" "pid $pg_pid gone"
  fi
  # After: the next command sees the new universe — clean analyze, dep a
  # work point.
  run_dartq_in pg-packages "$base/pg" packages --format json
  assert_eq "$DQ_RC" 0 "packages reads the post-pub-get universe"
  assert_contains "$(cat "$(out_file pg-packages)")" '"kind":"path-dependency"' \
    "the path dependency is now a work point (kinded by its consumer)"
  assert_contains "$(cat "$(out_file pg-packages)")" '"name":"dep"' \
    "the dependency names itself"
  run_dartq_in pg-after "$base/pg" analyze --sdk-path "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the once-dangling import now resolves (no stale universe)"
  # The resident engine answers over the same changed tree.
  cat >"$EVIDENCE/pg-mcp.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {}},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false", "result.content.0.text contains=diags"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/pg" "$DARTQ_DART_SDK" \
    "$EVIDENCE/pg-mcp.json" >"$EVIDENCE/pg-mcp.log" 2>&1
  assert_eq "$?" 0 "the resident engine also answers over the changed universe"
  run_dartq_in pg-stop "$base/pg" session stop "$base/pg"
  assert_eq "$DQ_RC" 0 "cross teardown"
}
