# legs/09-exit-codes.sh — the documented exit tiers per verb: the
# analyze severity ladder (0 clean, 1 fatal-infos, 2 fatal-warnings,
# 3 errors) driven hermetically through analysis_options severity remaps,
# usage errors (64) for every verb's malformed face, and the version
# faces.

legs_09_run() {
  local base="$E2E_WORK/rc"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"

  # The severity ladder: one diagnostic, three severities via remap.
  local kind
  for kind in info warning error; do
    make_pkg "$base/$kind" "rc_$kind"
    wf "$base/$kind/analysis_options.yaml" "analyzer:
  errors:
    unused_local_variable: $kind
"
    wf "$base/$kind/lib/main.dart" 'void main() {
  var unused = 1;
}
'
  done

  leg_begin rc-ladder "the analyze severity ladder (0/1/2/3) with fatal flags"
  # Info severity: clean by default, 1 under --fatal-infos.
  run_dartq_in rc-info "$base/info" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 0 "an info-only tree exits 0 by default"
  run_dartq_in rc-info-fatal "$base/info" analyze --sdk-path "$base/sdk" --fatal-infos lib
  assert_eq "$DQ_RC" 1 "an info-only tree exits 1 under --fatal-infos"
  # Warning severity: 2 by default (fatal warnings), 0 negated.
  run_dartq_in rc-warn "$base/warning" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 2 "a warning-only tree exits 2 by default"
  run_dartq_in rc-warn-no "$base/warning" analyze --sdk-path "$base/sdk" --no-fatal-warnings lib
  assert_eq "$DQ_RC" 0 "a warning-only tree exits 0 under --no-fatal-warnings"
  # Error severity: always 3.
  run_dartq_in rc-err "$base/error" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 3 "an error tree exits 3"
  run_dartq_in rc-err-no "$base/error" analyze --sdk-path "$base/sdk" --no-fatal-warnings lib
  assert_eq "$DQ_RC" 3 "errors stay fatal regardless of the warning flags"
  # The syntax-error face without any options file is also tier 3.
  # (Recovery shapes with an unbalanced/missing body delimiter —
  # `void main( {}`, `void main() {` — used to trip a source-range
  # panic in recovery and exit 4; the position conversion is total since
  # #2589, pinned by dartq-commands' recovery-shape writer test. The
  # fixture here keeps the missing-token shape that reports cleanly.)
  make_pkg "$base/syn" "rc_syn"
  wf "$base/syn/lib/main.dart" 'int main() => 1 oops;
'
  run_dartq_in rc-syn "$base/syn" analyze --sdk-path "$base/sdk" lib
  assert_eq "$DQ_RC" 3 "a parser error exits 3"

  leg_begin rc-usage "usage errors are 64 across the verbs"
  # analyze: a missing target.
  run_dartq_in rc-an-missing "$base/syn" analyze /nonexistent/dq_e2e
  assert_eq "$DQ_RC" 64 "analyze on a missing target is usage"
  # query: a missing dir (no pubspec → no workspace).
  run_dartq rc-q-missing query symbol nope "$base/rc-missing-dir"
  assert_eq "$DQ_RC" 64 "query on a missing dir is usage"
  # explain / impact / compat / resolve / complete / rename: missing files.
  run_dartq rc-explain explain nosuch /nonexistent/dq_e2e
  assert_eq "$DQ_RC" 64 "explain on a missing dir is usage"
  run_dartq_in rc-impact "$base/syn" impact lib/gone.dart --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "impact on a missing file is usage"
  run_dartq_in rc-compat "$base/syn" compat lib/gone.dart --sdk-root "$base/sdk"
  assert_eq "$DQ_RC" 64 "compat on a missing file is usage"
  run_dartq_in rc-rename "$base/syn" rename "lib/gone.dart:1:1" --new-name x --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "rename on a missing file is usage"
  # session: no SDK source at all.
  local sess_rc=0
  ( cd "$base/syn" && unset DART_SDK && exec "$DARTQ" session start "$base/syn" ) \
    >"$EVIDENCE/rc-sess.out" 2>"$EVIDENCE/rc-sess.err" || sess_rc=$?
  assert_eq "$sess_rc" 64 "session start without any SDK source is usage"
  # events: an unknown handle.
  run_dartq_in rc-events "$base/syn" events bogus "$base/syn"
  assert_eq "$DQ_RC" 64 "events with an unknown handle is usage"
  # fix: a missing target.
  run_dartq_in rc-fix "$base/syn" fix /nonexistent/dq_e2e
  assert_eq "$DQ_RC" 64 "fix on a missing target is usage"
  # mcp: an unresolvable dir.
  run_dartq rc-mcp mcp "$base/gone-too"
  assert_eq "$DQ_RC" 64 "mcp on a missing dir is usage"
  # The top level: an unknown verb is the clap usage error.
  run_dartq rc-unknown-verb no-such-verb
  assert_eq "$DQ_RC" 64 "an unknown verb is usage"
  run_dartq rc-unknown-flag analyze --bogus-flag lib
  assert_eq "$DQ_RC" 64 "an unknown flag is usage"

  leg_begin rc-version "the version faces exit 0"
  run_dartq rc-version version
  assert_eq "$DQ_RC" 0 "dartq version exits 0"
  assert_contains "$(cat "$(out_file rc-version)")" "dartq " "the version line prints"
  run_dartq rc-long-version --version
  assert_eq "$DQ_RC" 0 "--version exits 0"
  run_dartq rc-bare
  assert_eq "$DQ_RC" 0 "bare dartq exits 0 (the version fallback)"
}
