# legs/10-debug.sh — the debug verb group end to end: the proxy tiers
# over a really-served session (the empty registry, the domain miss as
# data at rc 1, usage at 64), auto-attach from deep and relocated trees,
# and — with a real Dart SDK — the launch loop itself: paused-on-start
# takeover, breakpoint hit, frame evaluation, stepping, hot reload,
# detach-leaves-it-alive, re-attach, stop (zero orphan processes).

# Polls a dartq debug read until its output carries NEEDLE, then echoes
# the matching output; gives up after ~10 s and returns 1. Used for the
# free-running program's async evidence (output events land after the
# reads begin).
dbg_poll() { # ID SECONDS ARGS... — uses run_dartq with the id prefix
  local id="$1" secs="$2"; shift 2
  local deadline=$((SECONDS + secs))
  while [ "$SECONDS" -lt "$deadline" ]; do
    run_dartq "$id-poll" "$@" >/dev/null
    if grep -q "$DBG_NEEDLE" "$EVIDENCE/$id-poll.out" 2>/dev/null; then
      cat "$EVIDENCE/$id-poll.out"
      return 0
    fi
    sleep 0.3
  done
  return 1
}

legs_10_run() {
  local base="$E2E_WORK/debug"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"

  # --- 10.1 the proxy tiers over a really-served session ---------------
  leg_begin debug-tiers "status verbatim, the domain miss as data at 1, usage at 64"
  make_pkg "$base/w1" "w1"
  wf "$base/w1/lib/main.dart" 'int main() => 1;
'
  run_dartq dbg-start session start "$base/w1" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the session starts"
  run_dartq dbg-status debug status --dir "$base/w1" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "status on the empty registry is clean"
  assert_eq "$(cat "$(out_file dbg-status)")" "[]" \
    "status prints the daemon answer verbatim — one JSON line"
  run_dartq dbg-miss debug status --sessionId 7 --dir "$base/w1" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 1 "the machine-coded miss is the domain tier"
  json_check "$(out_file dbg-miss)" \
    "j['error']['code']==-32001 and j['error']['message'].startswith('DARTQ/DEBUG/NO_SUCH_SESSION')" \
    "the error line carries the -32001 code and the DARTQ/DEBUG machine code"
  run_dartq dbg-detach-miss debug detach --sessionId 7 --dir "$base/w1" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 1 "detach of an unknown session is the same domain tier"
  run_dartq dbg-step-bad debug step --sessionId 1 sideways --dir "$base/w1"
  assert_eq "$DQ_RC" 64 "an unknown step option is usage"
  run_dartq dbg-bogus debug bogus --dir "$base/w1"
  assert_eq "$DQ_RC" 64 "an unknown subverb is usage"
  run_dartq dbg-gone debug status --dir "$base/gone"
  assert_eq "$DQ_RC" 64 "an unresolvable worktree is usage"
  run_dartq dbg-stop session stop "$base/w1"
  assert_eq "$DQ_RC" 0 "tier teardown"

  # --- 10.2 auto-attach from a relocated long path and a member dir ----
  leg_begin debug-nested "auto-attach + debug from a SUN_LEN-relocated tree and a workspace member"
  # A worktree whose local socket path exceeds sun_path: the socket
  # relocates (#2592) and the first debug command — no prior session
  # start — elects the server itself (the auto-attach contract).
  local segment long_root
  segment="$(printf 'w%.0s' $(seq 1 72))"
  long_root="$base/long-$segment"
  make_pkg "$long_root" "longpkg"
  wf "$long_root/lib/main.dart" 'int main() => 1;
'
  run_dartq_in dbg-long-status "$long_root/lib" debug status --dir "$long_root" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the relocated-tree session answers"
  assert_eq "$(cat "$(out_file dbg-long-status)")" "[]" "its empty debug registry lists empty"
  if [ -e "$long_root/.dart_tool/dartq/session.sock.point" ]; then
    ok "the long tree's socket relocated (the pointer file exists)"
  else
    bad "the long tree's socket relocated" "no session.sock.point under the long root"
  fi
  run_dartq dbg-long-stop session stop "$long_root"
  assert_eq "$DQ_RC" 0 "long-tree teardown"

  # A pub-workspace member: the member is its own resolution root, and
  # the debug verb run from inside the member elects + attaches there.
  local ws="$base/ws" member="$base/ws/packages/member"
  wf "$ws/pubspec.yaml" 'name: ws
workspace:
  - packages/member
environment:
  sdk: ^3.5.0
'
  make_pkg "$member" "member"
  wf "$member/lib/main.dart" 'int main() => 1;
'
  run_dartq_in dbg-member-status "$member" debug status --dir . --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the member-subdirectory session answers"
  assert_eq "$(cat "$(out_file dbg-member-status)")" "[]" "the member's empty debug registry lists empty"
  run_dartq dbg-member-stop session stop "$member"
  assert_eq "$DQ_RC" 0 "member-tree teardown"

  # --- 10.3 the real dart launch loop (needs the real SDK) -------------
  leg_begin debug-dart-loop "launch paused → breakpoint → evaluate → step → reload → detach → re-attach → stop"
  if [ ! -x "$DARTQ_DART_SDK/bin/dart" ]; then
    skip "the dart launch loop needs DARTQ_DART_SDK (default ~/fvm/default)"
    return
  fi
  local tree="$base/loop" dart_bin="$DARTQ_DART_SDK/bin/dart"
  make_pkg "$tree" "loop"
  # The reload target is a package library — the VM does not hot-reload
  # a bare script's own root library, so the changing string lives in
  # lib/ and the entry script imports it (the supported edit-reload
  # path; the entry's own edits need a restart).
  wf "$tree/lib/msg.dart" "String message() => 'v1';
"
  wf "$tree/bin/loop.dart" "import 'package:loop/msg.dart';

int tick(int n) => n + 1;

void main() {
  print('dart-loop-ready:\${message()}');
  int n = 0;
  while (true) {
    n = tick(n);
    if (n % 10000000 == 0) print('tick=\$n \${message()}');
  }
}
"
  # The serve child resolves `dart` through DARTQ_DART_BIN; export ahead
  # of the election so the spawned server inherits it.
  export DARTQ_DART_BIN="$dart_bin"
  run_dartq dbg-loop-start session start "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the loop tree's session starts on the real SDK"

  # Paused-on-start takeover: launch → arm → resume.
  run_dartq dbg-launch debug launch bin/loop.dart --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "launch answers clean"
  json_check "$(out_file dbg-launch)" \
    "j['sessionId']>0 and len(j['isolateIds'])>0 and j['mainIsolate']" \
    "the launch answer carries the session id and the isolate set"
  local sid isolate bpid
  sid="$(python3 -c "import json; print(json.load(open('$(out_file dbg-launch)'))['sessionId'])")"
  isolate="$(python3 -c "import json; print(json.load(open('$(out_file dbg-launch)'))['mainIsolate'])")"
  run_dartq dbg-bp-add debug breakpoint add --sessionId "$sid" \
    --script "file://$tree/bin/loop.dart" --line 3 --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the line breakpoint arms while paused"
  json_check "$(out_file dbg-bp-add)" "len(j['id'])>0" "the arm answer carries the breakpoint id"
  bpid="$(python3 -c "import json; print(json.load(open('$(out_file dbg-bp-add)'))['id'])")"
  run_dartq dbg-resume debug resume --sessionId "$sid" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "resume releases the start pause"
  # The program hits the breakpoint immediately (tick is called in a
  # tight loop) — the snapshot waits for the pause, bounded by the
  # session config.
  run_dartq dbg-snap debug snapshot --sessionId "$sid" --isolate "$isolate" \
    --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the app hits the breakpoint; the snapshot answers"
  json_check "$(out_file dbg-snap)" \
    "j['reason']['kind']=='breakpoint' and j['location']['line']==3" \
    "the stop names the breakpoint at line 3"
  run_dartq dbg-eval debug evaluate --sessionId "$sid" --frameIndex 0 'n + 40' \
    --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "evaluation in the paused frame answers"
  json_check "$(out_file dbg-eval)" "j['value']=='40'" "the frame's n is 0; n + 40 evaluates to 40"
  run_dartq dbg-step debug step --sessionId "$sid" over --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "step-over answers with the next snapshot"
  json_check "$(out_file dbg-step)" "j['reason']['kind']=='step'" "the stop names the step"
  run_dartq dbg-exc debug exception-pause --sessionId "$sid" uncaught --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the exception pause mode sets on the stopped isolate"
  run_dartq dbg-bp-rm debug breakpoint rm --sessionId "$sid" --breakpointId "$bpid" \
    --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the breakpoint removes"
  run_dartq dbg-resume2 debug resume --sessionId "$sid" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the program free-runs"

  # Hot reload: edit the package library, reload, and the program's
  # next print carries the change (the edit-reload half of the loop).
  wf "$tree/lib/msg.dart" "String message() => 'v2';
"
  run_dartq dbg-reload debug reload --sessionId "$sid" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "hot reload answers"
  json_check "$(out_file dbg-reload)" "j['ok']==True" "the reload succeeded"
  DBG_NEEDLE="v2"
  if dbg_poll dbg-out 15 debug output --sessionId "$sid" --dir "$tree" --sdk "$DARTQ_DART_SDK"; then
    ok "the reloaded program prints v2 (edit → reload took effect)"
  else
    bad "the reloaded program prints v2" "no v2 line within 15 s: $(tail -c 200 "$EVIDENCE/dbg-out-poll.out" 2>/dev/null)"
  fi
  run_dartq dbg-events debug events --sessionId "$sid" --limit 8 --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "the cursor-paged event read answers"
  json_check "$(out_file dbg-events)" "len(j['events'])>0 and 'nextCursor' in j" \
    "the events page carries entries and the next cursor"

  # Endings, per the ownership contract: detach leaves the program
  # running (the ws URI answers); a re-attach reaches it but its stop
  # ends the debug session only (an attached program was never ours);
  # the zero-orphan guarantee is a launched session stopped directly.
  run_dartq dbg-detach debug detach --sessionId "$sid" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "detach answers"
  local wsuri
  wsuri="$(python3 -c "import json; print(json.load(open('$(out_file dbg-detach)'))['wsUri'])")"
  case "$wsuri" in
    ws://*) ok "detach returns the re-attach handle ($wsuri)" ;;
    *) bad "detach returns the re-attach handle" "wsUri: [$wsuri]" ;;
  esac
  sleep 0.5
  if ps -axo command= | grep -F "$tree/bin/loop.dart" | grep -v grep >/dev/null; then
    ok "the detached program keeps running"
  else
    bad "the detached program keeps running" "no bin/loop.dart process after detach"
  fi
  run_dartq dbg-attach debug attach --wsUri "$wsuri" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "re-attach reaches the still-running program"
  json_check "$(out_file dbg-attach)" "j['sessionId']>$sid" "the re-attach owns a fresh session id"
  local sid2
  sid2="$(python3 -c "import json; print(json.load(open('$(out_file dbg-attach)'))['sessionId'])")"
  run_dartq dbg-stop-reattach debug stop --sessionId "$sid2" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "stop on the re-attached session answers"
  sleep 0.5
  if ps -axo command= | grep -F "$tree/bin/loop.dart" | grep -v grep >/dev/null; then
    ok "an attach-origin stop ends the session, not the program (the ownership contract)"
  else
    bad "an attach-origin stop ends the session, not the program" "the program died with the re-attach stop"
  fi
  # The post-detach program is the caller's now — take it down the way
  # any owner would, then prove the zero-orphan stop path on a fresh
  # launched session.
  pkill -f "$tree/bin/loop.dart" 2>/dev/null || true
  run_dartq dbg-launch2 debug launch bin/loop.dart --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "a second launch opens a second session"
  local sid3
  sid3="$(python3 -c "import json; print(json.load(open('$(out_file dbg-launch2)'))['sessionId'])")"
  run_dartq dbg-stop2 debug stop --sessionId "$sid3" --dir "$tree" --sdk "$DARTQ_DART_SDK"
  assert_eq "$DQ_RC" 0 "stop on the launched session answers"
  sleep 1
  if ps -axo command= | grep -F "$tree/bin/loop.dart" | grep -v grep >/dev/null; then
    bad "stop reaps the launched program" "a process still runs bin/loop.dart"
  else
    ok "stop reaps the launched program (zero orphans)"
  fi
  run_dartq dbg-loop-stop session stop "$tree"
  assert_eq "$DQ_RC" 0 "loop teardown"
}
