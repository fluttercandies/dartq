# legs/11-routed-parity.sh — the routed-vs-cold CI goldens (#2631): the
# resident consult's whole contract at process level, over a real fixture
# and the release binary —
#   * `analyze` routed (the default) answers the `--cold` bytes exactly,
#     on every format and exit code;
#   * a target filter and multiple targets stay filtered and byte-equal;
#   * killing the daemon between calls loses nothing: the next call
#     answers (re-election or fallback) and the bytes still equal the
#     cold bytes.

# bytes_equal ID_A ID_B DESCRIPTION — the two captured stdout files are
# byte-identical.
bytes_equal() {
  if cmp -s "$EVIDENCE/$1.out" "$EVIDENCE/$2.out"; then
    ok "$3"
  else
    bad "$3" "$(printf '%s' "differ: $(head -c 200 "$EVIDENCE/$1.out")" ) vs $(printf '%s' "$(head -c 200 "$EVIDENCE/$2.out")")"
  fi
}

# bytes_differ ID_A ID_B DESCRIPTION — the two captured stdout files are
# NOT byte-identical (the filter actually filtered).
bytes_differ() {
  if cmp -s "$EVIDENCE/$1.out" "$EVIDENCE/$2.out"; then
    bad "$1" "unexpectedly equal to $2"
  else
    ok "$3"
  fi
}

legs_11_run() {
  local base="$E2E_WORK/routed"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"

  # --- 11.1 routed (default) == --cold, every format --------------------
  leg_begin routed-parity-bytes "analyze routed (default) == --cold, byte for byte, every format"
  make_pkg "$base/w1" "w1"
  # A clean library plus a semantically-broken one (the resident tests'
  # own shape — a parse-only fixture can recover clean through the engine
  # face): the diagnostics keep the byte identity honest.
  wf "$base/w1/lib/main.dart" 'int main() => 1;
'
  wf "$base/w1/lib/broken.dart" 'void f() { x }
'
  for format in lon json text; do
    run_dartq_in "r11-cold-$format" "$base/w1" analyze --sdk-path "$base/sdk" --cold --format "$format" . >/dev/null
    local cold_rc=$DQ_RC
    run_dartq_in "r11-routed-$format" "$base/w1" analyze --sdk-path "$base/sdk" --format "$format" . >/dev/null
    assert_eq "$DQ_RC" "$cold_rc" "exit tier matches ($format)"
    bytes_equal "r11-cold-$format" "r11-routed-$format" "routed stdout == cold stdout ($format)"
  done
  if [ -s "$EVIDENCE/r11-cold-lon.out" ]; then
    ok "the fixture keeps the bytes honest (diagnostics present)"
  else
    bad "the fixture keeps the bytes honest" "cold lon output is empty"
  fi
  run_dartq_in r11-stop-1 "$base/w1" session stop "$base/w1" --format json >/dev/null || true

  # --- 11.2 target filter and multiple targets ---------------------------
  leg_begin routed-parity-targets "a filtered target and multiple targets stay filtered and byte-equal"
  run_dartq_in r11-cold-one "$base/w1" analyze --sdk-path "$base/sdk" --cold lib/main.dart >/dev/null
  local cold_one_rc=$DQ_RC
  run_dartq_in r11-routed-one "$base/w1" analyze --sdk-path "$base/sdk" lib/main.dart >/dev/null
  assert_eq "$DQ_RC" "$cold_one_rc" "the single-file target's exit tier matches"
  bytes_equal r11-cold-one r11-routed-one "the single-file target answers the cold bytes"
  bytes_differ r11-cold-one r11-cold-lon "the single-file target actually filters (differs from the whole tree)"

  run_dartq_in r11-cold-many "$base/w1" analyze --sdk-path "$base/sdk" --cold lib/main.dart lib/broken.dart >/dev/null
  local cold_many_rc=$DQ_RC
  run_dartq_in r11-routed-many "$base/w1" analyze --sdk-path "$base/sdk" lib/main.dart lib/broken.dart >/dev/null
  assert_eq "$DQ_RC" "$cold_many_rc" "the multi-target exit tier matches"
  bytes_equal r11-cold-many r11-routed-many "the multi-target run answers the cold bytes"
  run_dartq_in r11-stop-2 "$base/w1" session stop "$base/w1" --format json >/dev/null || true

  # --- 11.3 kill the daemon between calls --------------------------------
  leg_begin routed-parity-kill "kill the daemon between calls: the next call still answers the cold bytes"
  run_dartq_in r11-kill-start "$base/w1" session start "$base/w1" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "the doomed session starts"
  local dead_pid
  dead_pid="$(session_pid "$base/w1")"
  kill -9 "$dead_pid"
  # A just-killed detached serve reports alive until its reaper collects
  # the zombie; poll briefly before judging.
  local waited=0
  while kill -0 "$dead_pid" 2>/dev/null && [ "$waited" -lt 20 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  if kill -0 "$dead_pid" 2>/dev/null; then
    bad "the daemon is dead" "pid $dead_pid still alive"
  else
    ok "the daemon is dead"
  fi
  # The next default call answers over the killed session's residue — the
  # ladder re-elects (or falls back); either way the bytes are the cold
  # bytes and the exit tier is the cold tier.
  run_dartq_in r11-after-kill "$base/w1" analyze --sdk-path "$base/sdk" --format lon . >/dev/null
  assert_eq "$DQ_RC" "$(cat "$EVIDENCE/r11-cold-lon.rc")" "the post-kill exit tier matches cold"
  bytes_equal r11-after-kill r11-cold-lon "the post-kill answer equals the cold bytes"
  run_dartq_in r11-stop-3 "$base/w1" session stop "$base/w1" --format json >/dev/null || true
}
