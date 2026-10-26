# legs/02-session.sh — the resident session layer end to end: start →
# attach (one engine per worktree) → status → stop; two concurrent
# sessions in two checkouts; kill -9 stale-socket takeover; LRU eviction
# past the cap then cold recovery; a read-only checkout degrading to
# standalone with a stated reason.

legs_02_run() {
  local base="$E2E_WORK/sess"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"

  # --- 02.1 start → attach → status → stop ------------------------------
  leg_begin sess-lifecycle "start, attach, status, stop over one worktree"
  make_pkg "$base/w1" "w1"
  wf "$base/w1/lib/main.dart" 'int main() => 1;
'
  run_dartq_in sess-start "$base/w1" session start "$base/w1" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "session start exits clean"
  json_check "$(out_file sess-start)" "j['type']=='session' and j['attached']==True" \
    "start attaches (the json row channel)"
  json_check "$(out_file sess-start)" "j['pid']>0 and len(j['socket'])>0" \
    "the session row carries pid and socket"
  local pid1
  pid1="$(session_pid "$base/w1")"

  # A second starter attaches the same server (the election's live path).
  run_dartq_in sess-attach "$base/w1" session start "$base/w1" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "the second start attaches"
  json_check "$(out_file sess-attach)" "j['type']=='session' and j['attached']==True and j['pid']==$pid1" \
    "the same pid — one engine per worktree, not a second server"

  # status lists the live session for this checkout.
  run_dartq_in sess-status "$base/w1" session status --format json
  assert_eq "$DQ_RC" 0 "status exits clean"
  assert_eq "$(jsonl_count "$(out_file sess-status)" session)" 1 \
    "status shows one live session row"

  # stop cleans: process gone, socket and state files removed.
  run_dartq_in sess-stop "$base/w1" session stop "$base/w1" --format json
  assert_eq "$DQ_RC" 0 "stop exits clean"
  json_check "$(out_file sess-stop)" "j['type']=='stopped'" "stop prints the stopped row"
  if kill -0 "$pid1" 2>/dev/null; then
    bad "the serve process is gone after stop" "pid $pid1 still alive"
  else
    ok "the serve process is gone after stop"
  fi
  if [ -e "$base/w1/.dart_tool/dartq/session.sock" ] || [ -e "$base/w1/.dart_tool/dartq/state.json" ]; then
    bad "stop removes the socket and state files" "residue under .dart_tool/dartq"
  else
    ok "stop removes the socket and state files"
  fi

  # --- 02.2 two concurrent sessions in two checkouts ---------------------
  leg_begin sess-two-checkouts "two concurrent sessions in two checkouts"
  make_pkg "$base/wa" "wa"
  make_pkg "$base/wb" "wb"
  run_dartq_in sess-two-a "$base/wa" session start "$base/wa" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "checkout A starts"
  run_dartq_in sess-two-b "$base/wb" session start "$base/wb" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "checkout B starts (A stays live — different roots)"
  local pida pidb
  pida="$(session_pid "$base/wa")"
  pidb="$(session_pid "$base/wb")"
  if [ "$pida" != "$pidb" ]; then
    ok "the two checkouts hold distinct servers ($pida vs $pidb)"
  else
    bad "the two checkouts hold distinct servers" "same pid $pida"
  fi
  run_dartq_in sess-two-status "$base/wa" session status --format json
  assert_eq "$(jsonl_count "$(out_file sess-two-status)" session)" 2 \
    "status lists both live sessions"
  run_dartq_in sess-two-stopall "$base/wa" session stop --all --format json
  assert_eq "$DQ_RC" 0 "stop --all exits clean"
  json_check "$(out_file sess-two-stopall)" "j['type']=='stopped' and j['count']>=2" \
    "stop --all reports both sessions"
  for p in "$pida" "$pidb"; do
    if kill -0 "$p" 2>/dev/null; then
      bad "stop --all killed every server" "pid $p still alive"
    else
      ok "stop --all killed pid $p"
    fi
  done

  # --- 02.3 kill -9: stale socket takeover ------------------------------
  leg_begin sess-stale-takeover "kill -9 the engine, the next command recovers"
  run_dartq_in sess-kill-start "$base/w1" session start "$base/w1" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "the doomed session starts"
  local dead_pid
  dead_pid="$(session_pid "$base/w1")"
  kill -9 "$dead_pid"
  # The crashed residue: a socket file naming a dead server — the
  # takeover input the next command must recover from.
  if [ -e "$base/w1/.dart_tool/dartq/session.sock" ]; then
    ok "the crashed server leaves its socket file"
  else
    bad "the crashed server leaves its socket file" "socket already gone"
  fi
  run_dartq_in sess-takeover "$base/w1" session start "$base/w1" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the next start recovers"
  assert_contains "$(cat "$(out_file sess-takeover)")" "type:session" \
    "the default lon face prints the attach row"
  run_dartq_in sess-takeover-json "$base/w1" session start "$base/w1" --sdk "$base/sdk" --format json
  json_check "$(out_file sess-takeover-json)" "j['type']=='session' and j['attached']==True and j['pid']!=$dead_pid" \
    "a fresh server took over the stale socket"
  run_dartq_in sess-takeover-stop "$base/w1" session stop "$base/w1"
  assert_eq "$DQ_RC" 0 "takeover teardown"

  # --- 02.4 LRU eviction past the cap, then cold recovery ---------------
  # The registry caps live sessions at 5. Six live servers mean the next
  # start evicts the least-recently-heartbeaten one — which root that is
  # belongs to the engine (heartbeats are its clock), so the leg reads
  # the evicted root from the evicted row instead of guessing it.
  leg_begin sess-lru-eviction "the session cap evicts the LRU server; the evicted root recovers cold"
  local i evicted_root evicted_pid
  for i in 1 2 3 4 5 6; do
    make_pkg "$base/lru$i" "lru$i"
    run_dartq_in "sess-lru-$i" "$base/lru$i" session start "$base/lru$i" --sdk "$base/sdk" --format json
    assert_eq "$DQ_RC" 0 "session $i starts"
  done
  # The 7th start (an attach of lru6) is what runs the cap enforcement.
  run_dartq_in sess-lru-evict "$base/lru6" session start "$base/lru6" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "the cap-enforcing start still attaches"
  if grep -q '"type":"evicted"' "$EVIDENCE/sess-lru-evict.out"; then
    ok "the start printed its eviction row"
  else
    bad "the start printed its eviction row" "no evicted row: $(cat "$EVIDENCE/sess-lru-evict.out")"
  fi
  evicted_root="$(jsonl_field "$(out_file sess-lru-evict)" evicted root | tr -d '"')"
  if [ -n "$evicted_root" ] && [ -d "$evicted_root" ]; then
    ok "the evicted row names a real root ($evicted_root)"
  else
    bad "the evicted row names a real root" "got [$evicted_root]"
  fi
  run_dartq_in sess-lru-status "$base/lru6" session status --format json
  assert_eq "$(jsonl_count "$(out_file sess-lru-status)" session)" 5 \
    "status holds exactly 5 live sessions (the cap)"
  # The evicted server is dead — its stale socket file stays on disk
  # until that root's next start takes it over (the cold-restart
  # contract; the recovery step below is that takeover).
  evicted_pid="$(python3 -c "
import json
try:
    print(json.load(open('$evicted_root/.dart_tool/dartq/state.json'))['pid'])
except (FileNotFoundError, ValueError):
    print('gone')
")"
  if [ "$evicted_pid" != "gone" ] && kill -0 "$evicted_pid" 2>/dev/null; then
    bad "the evicted server process is dead" "pid $evicted_pid still alive"
  else
    ok "the evicted server process is dead"
  fi
  # Cold recovery: the evicted root starts again from scratch.
  run_dartq_in sess-lru-recover "$evicted_root" session start "$evicted_root" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "the evicted root recovers cold"
  json_check "$(out_file sess-lru-recover)" "j['type']=='session' and j['attached']==True" \
    "recovery attaches a fresh server"
  run_dartq_in sess-lru-stop "$base/lru1" session stop --all
  assert_eq "$DQ_RC" 0 "LRU teardown"

  # --- 02.5 read-only checkout degrades with a stated reason ------------
  leg_begin sess-readonly "a read-only checkout degrades to standalone with a reason"
  make_pkg "$base/ro" "ro"
  chmod -R a-w "$base/ro"
  run_dartq_in sess-ro "$base/ro" session start "$base/ro" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "standalone is printed, never an error"
  json_check "$(out_file sess-ro)" "j['type']=='standalone' and len(j['reason'])>0" \
    "the degradation reason is stated"
  chmod -R u+w "$base/ro"
}
