# legs/07-mcp.sh — the MCP stdio face over a hermetic tree: the
# initialize handshake, tools/list (every description states when to use
# it and when not), real tool calls (analyze, query, events, rename),
# progress notifications when a progressToken rides the request, error
# shapes for unknown tools and methods, the workspace-status resource,
# and the clean EOF shutdown.

legs_07_run() {
  local base="$E2E_WORK/mcp"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"
  make_pkg "$base/w" "mcpw"
  wf "$base/w/lib/alpha.dart" 'int alpha() => 1;
'
  wf "$base/w/lib/main.dart" "import 'alpha.dart';

int main() => alpha();
"

  # One conversation covers the whole contract; the driver verdict lines
  # carry every expectation. The rename tool's cold engine reads $DART_SDK
  # (the server's --sdk flag does not reach it), so the script carries the
  # stub SDK through the driver's env injection; @SDK@ substitutes it.
  cat >"$EVIDENCE/mcp-script.json" <<'SCRIPT'
{"env": {"DART_SDK": "@SDK@"},
 "requests": [
  {"id": 1, "method": "initialize", "params": {},
   "expect": ["result.protocolVersion: \"2025-06-18\"",
              "result.serverInfo.name: \"dartq\"",
              "result.capabilities.tools exists"]},
  {"notify": true, "method": "initialized"},
  {"id": 2, "method": "ping", "expect": []},
  {"id": 3, "method": "tools/list",
   "expect": ["len(result['tools'])>=20",
              "all(any(t['name']==n for t in result['tools']) for n in ['dartq_analyze','dartq_query','dartq_test','dartq_test_status','dartq_events','dartq_rename','dartq_fix','dartq_impact'])",
              "all(('Use' in t['description'] and 'Not for' in t['description']) for t in result['tools'])"]},
  {"id": 4, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: false", "result.content.0.type: \"text\""]},
  {"id": 5, "method": "tools/call",
   "params": {"name": "dartq_query",
              "arguments": {"name": "alpha", "kind": "symbol"}},
   "expect": ["result.isError: false",
              "result.content.0.text contains=alpha"]},
  {"id": 6, "method": "tools/call",
   "params": {"name": "dartq_events", "arguments": {"handle": "progress"}},
   "expect": ["result.isError: false", "result.content.0.text contains=files_scanned"]},
  {"id": 7, "method": "tools/call",
   "params": {"name": "dartq_rename",
              "arguments": {"target": "@TREE@/lib/alpha.dart:1:5", "new_name": "zulu"}},
   "expect": ["result.isError: false",
              "result.content.0.text contains=zulu"]},
  {"id": 8, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {},
              "_meta": {"progressToken": "tok-1"}},
   "expect": ["result.isError: false"]},
  {"id": 9, "method": "tools/call",
   "params": {"name": "dartq_nope", "arguments": {}},
   "expect": ["code: -32602"]},
  {"id": 10, "method": "resources/list",
   "expect": ["result.resources.0.uri: \"dartq://workspace-status\""]},
  {"id": 11, "method": "resources/read",
   "params": {"uri": "dartq://workspace-status"},
   "expect": ["result.contents.0.mimeType: \"application/json\"",
              "result.contents.0.text contains=root"]},
  {"id": 12, "method": "no/such/method",
   "expect": ["code: -32601"]}
]}
SCRIPT

  leg_begin mcp-stdio "the MCP stdio conversation: handshake, tools, calls, resources"
  sed -i '' -e "s|@SDK@|$base/sdk|g" -e "s|@TREE@|$base/w|g" "$EVIDENCE/mcp-script.json"
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/w" "$base/sdk" \
    "$EVIDENCE/mcp-script.json" >"$EVIDENCE/mcp-driver.log" 2>&1
  assert_eq "$?" 0 "every scripted expectation held"
  assert_contains "$(cat "$EVIDENCE/mcp-driver.log")" '"pass": true' \
    "verdict lines all pass"
  # The rename tool is dry-run by contract: the file on disk is unchanged.
  assert_contains "$(cat "$base/w/lib/alpha.dart")" "alpha" \
    "dartq_rename applied nothing to disk (the caller applies edits)"
  # The conversation also proved progress notifications: the driver's
  # progress summary line must show the token bookends.
  assert_contains "$(cat "$EVIDENCE/mcp-driver.log")" '"id": "progress", "check": "notifications seen", "pass": 2' \
    "the progressToken call emitted its two bookend notifications"

  # Degraded standalone: a read-only tree still serves the local faces,
  # engine tools answer isError with the stated reason.
  leg_begin mcp-degraded "a read-only tree degrades: local faces work, engine tools say why"
  make_pkg "$base/ro" "mcp_ro"
  chmod -R a-w "$base/ro"
  cat >"$EVIDENCE/mcp-ro.json" <<'SCRIPT'
{"requests": [
  {"id": 1, "method": "initialize", "params": {},
   "expect": ["result.serverInfo.name: \"dartq\""]},
  {"id": 2, "method": "tools/call",
   "params": {"name": "dartq_analyze", "arguments": {}},
   "expect": ["result.isError: true",
              "result.content.0.text contains=no resident session"]},
  {"id": 3, "method": "tools/call",
   "params": {"name": "dartq_events", "arguments": {"handle": "session"}},
   "expect": ["result.isError: false"]},
  {"id": 4, "method": "resources/read",
   "params": {"uri": "dartq://workspace-status"},
   "expect": ["result.contents.0.text contains=packages"]}
]}
SCRIPT
  python3 "$E2E_ROOT/lib/mcp_driver.py" "$DARTQ" "$base/ro" "$base/sdk" \
    "$EVIDENCE/mcp-ro.json" >"$EVIDENCE/mcp-ro.log" 2>&1
  assert_eq "$?" 0 "the degraded conversation held"
  chmod -R u+w "$base/ro"

  # Usage: an unresolvable dir is the usage tier before any stdio turn.
  leg_begin mcp-usage "an unresolvable dir is the usage tier"
  run_dartq mcp-gone mcp "$base/gone"
  assert_eq "$DQ_RC" 64 "a missing dir refuses before serving"
}
