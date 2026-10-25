# legs/03-events.sh — the unified event faces: `dartq events session`
# cursor pagination over a really-served session (serving, client
# attach/detach records), `dartq events progress` on a bigger tree (the
# cold-analysis snapshot), and — with a real Dart SDK — a test run's
# handle driven through the MCP tools (run → status → events cursor).

legs_03_run() {
  local base="$E2E_WORK/events"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"

  # --- 03.1 the session handle: cursor pagination ------------------------
  leg_begin events-session-cursor "the session handle paginates by cursor"
  make_pkg "$base/ev" "ev"
  wf "$base/ev/lib/main.dart" 'int main() => 1;
'
  run_dartq_in ev-start "$base/ev" session start "$base/ev" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the session starts"
  # One client connection (an MCP analyze call) drives the lifecycle
  # records: serving → client_attached → client_detached.
  cat >"$EVIDENCE/ev-mcp.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {},
   "expect": ["result.protocolVersion: \"2025-06-18\""]},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/ev" "$base/sdk" \
    "$EVIDENCE/ev-mcp.json" >"$EVIDENCE/ev-mcp.log" 2>&1
  assert_eq "$?" 0 "the MCP client drove one analyze over the session"

  # The full read carries the lifecycle records; every line parses.
  run_dartq_in ev-full "$base/ev" events session "$base/ev" --limit 100
  assert_eq "$DQ_RC" 0 "the session handle reads clean"
  jsonl_check "$(out_file ev-full)" "'event' in j" "every event line carries its event name"
  local names total
  names="$(python3 -c "
import json
for line in open('$(out_file ev-full)'):
    if line.strip():
        print(json.loads(line).get('event', ''))
")"
  assert_contains "$names" "serving" "the serving record is present"
  assert_contains "$names" "client_attached" "a client attach was recorded"
  assert_contains "$names" "client_detached" "a client detach was recorded"

  # Pagination: limit 1 gives exactly the first record and its seq; the
  # --since read returns exactly the remainder, all past the cursor.
  run_dartq_in ev-page1 "$base/ev" events session "$base/ev" --limit 1
  assert_eq "$(jsonl_count "$(out_file ev-page1)")" 1 "limit 1 reads one event"
  total="$(jsonl_count "$(out_file ev-full)")"
  first_seq="$(python3 -c "
import json
print(json.loads(open('$(out_file ev-page1)').read())['seq'])
")"
  run_dartq_in ev-rest "$base/ev" events session "$base/ev" --since "$first_seq" --limit 100
  assert_eq "$(jsonl_count "$(out_file ev-rest)")" "$((total - 1))" \
    "--since <seq> returns exactly the remaining events"
  jsonl_check "$(out_file ev-rest)" "j['seq']>$first_seq" "every paged event's seq is past the cursor"
  run_dartq_in ev-stop "$base/ev" session stop "$base/ev"
  assert_eq "$DQ_RC" 0 "events teardown"

  # --- 03.2 the progress handle on a bigger tree ------------------------
  leg_begin events-progress "cold-analysis progress appears for a bigger tree"
  make_pkg "$base/big" "big"
  gen_files "$base/big" 800
  run_dartq_in big-start "$base/big" session start "$base/big" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the big-tree session starts"
  run_dartq_in big-progress-0 "$base/big" events progress "$base/big"
  assert_eq "$DQ_RC" 0 "a pre-analysis progress read is clean (may print nothing)"
  cat >"$EVIDENCE/big-mcp.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {},
   "expect": ["result.protocolVersion: \"2025-06-18\""]},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/big" "$base/sdk" \
    "$EVIDENCE/big-mcp.json" >"$EVIDENCE/big-mcp.log" 2>&1
  assert_eq "$?" 0 "the cold analysis ran over the resident engine"
  run_dartq_in big-progress "$base/big" events progress "$base/big"
  assert_eq "$DQ_RC" 0 "the progress handle reads clean"
  json_check "$(out_file big-progress)" \
    "j['type']=='progress' and j['files_scanned']>=800 and j['cursor']==j['files_scanned']" \
    "the snapshot counts the swept tree and echoes the cursor"
  json_check "$(out_file big-progress)" "j['current_package'] is not None" \
    "the current package is named"
  run_dartq_in big-progress-done "$base/big" events progress "$base/big" --since 999999
  assert_eq "$DQ_RC" 0 "a past-end cursor reads clean"
  assert_eq "$(wc -c <"$(out_file big-progress-done)" | tr -d ' ')" 0 \
    "a past-end cursor prints nothing"
  run_dartq_in big-stop "$base/big" session stop "$base/big"
  assert_eq "$DQ_RC" 0 "big-tree teardown"

  # --- 03.3 a test run's handle (needs a real Dart SDK) ------------------
  leg_begin events-test-run "a test run's handle: run → status → events over the session"
  if [ ! -x "$DARTQ_DART_SDK/bin/dart" ]; then
    skip "real Dart SDK not found at $DARTQ_DART_SDK (set DARTQ_DART_SDK)"
    return 0
  fi
  local tf="$base/testface"
  cp -R "$REPO_ROOT/tests/testface/dart_pkg" "$tf"
  run_dartq_in tf-start "$tf" session start "$tf" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the testface session starts on the real SDK"
  # The tool result body is the daemon answer stringified into the text
  # content field: runId / exitCode live inside result.content[0].text.
  cat >"$EVIDENCE/tf-mcp.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {},
   "expect": ["result.protocolVersion: \"2025-06-18\""]},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_test",
              "arguments": {"args": ["test/pass_test.dart"]}},
   "expect": ["result.isError: false",
              "result.content.0.text contains=runId"]},
  {"id": 3, "method": "tools/call",
   "params": {"name": "dartq_test_status", "arguments": {}},
   "repeat_until": {"when": ["result.content.0.text contains=exitCode"],
                    "tries": 60, "delay_s": 2.0},
   "expect": ["result.isError: false",
              "result.content.0.text contains=exitCode"]},
  {"id": 4, "method": "tools/call",
   "params": {"name": "dartq_events",
              "arguments": {"handle": "session", "limit": 2}},
   "expect": ["result.isError: false"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$tf" "$DARTQ_DART_SDK" \
    "$EVIDENCE/tf-mcp.json" >"$EVIDENCE/tf-mcp.log" 2>&1
  assert_eq "$?" 0 "the MCP test-run conversation held (run → finished status → events)"
  assert_contains "$(cat "$EVIDENCE/tf-mcp.log")" '"pass": true' \
    "every scripted expectation passed"
  # The same log reads through the CLI face, cursor-paginated.
  run_dartq_in tf-cli-events "$tf" events session "$tf" --limit 3
  assert_eq "$DQ_RC" 0 "the CLI events face reads the same log"
  run_dartq_in tf-stop "$tf" session stop "$tf"
  assert_eq "$DQ_RC" 0 "testface teardown"
}
