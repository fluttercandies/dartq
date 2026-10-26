# legs/06-fix-rename.sh — the writing verbs: `dartq fix` (apply is the
# default) rewrites files in place and records every row's outcome; an
# anchor a concurrent edit erases is refused as `stale` and the on-disk
# edit survives untouched; `dartq rename --write` applies its edit set and
# refuses anything outside the workspace.
#
# fix takes no --sdk flag (its engine comes from $DART_SDK), so this leg
# exports the stub SDK for its duration.

legs_06_run() {
  local base="$E2E_WORK/write"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"
  export DART_SDK="$base/sdk"

  # --- fix --apply: the clean apply ------------------------------------
  leg_begin fix-apply "fix applies the unused-import removal and re-analyzes clean"
  make_pkg "$base/fx" "fx"
  wf "$base/fx/lib/helper.dart" 'int helper() => 1;
'
  wf "$base/fx/lib/main.dart" "import 'helper.dart';

int main() => 2;
"
  run_dartq_in fix-dry "$base/fx" fix --dry-run lib
  assert_eq "$DQ_RC" 0 "the dry run proposes without writing"
  assert_contains "$(cat "$(out_file fix-dry)")" 'edits:[' \
    "the lon document carries the edit set"
  assert_contains "$(cat "$base/fx/lib/main.dart")" "import 'helper.dart';" \
    "the dry run left the file untouched"

  run_dartq_in fix-apply "$base/fx" fix lib
  assert_eq "$DQ_RC" 0 "the apply run exits clean"
  assert_contains "$(cat "$base/fx/lib/main.dart")" "int main() => 2;" \
    "the file was rewritten in place"
  assert_not_contains "$(cat "$base/fx/lib/main.dart")" "import 'helper.dart';" \
    "the unused import is gone"
  run_dartq_in fix-recheck "$base/fx" analyze lib
  assert_eq "$DQ_RC" 0 "the post-fix tree analyzes clean"

  # --- fix: changed-under-us is refused, not clobbered -------------------
  # The drift-correction contract: an edit whose anchor no longer holds
  # anywhere in the apply-time text is refused as `stale`, and the bytes
  # on disk stay exactly what the concurrent editor wrote.
  #
  # The concurrent write must land between the collect stage's read of
  # main.dart and the apply stage's re-read of it. The fixture engineers
  # that window open: `analysis_options.yaml` excludes lib/src from the
  # engine pass (the pre-window — spawn, assembly, a two-file engine pass,
  # the loop's own prefix — measures ~10 ms) while its `linter.rules`
  # entry keeps the collect loop's enable set non-none, so every filler
  # file under lib/src is stat+read+parsed by the loop (~0.25 s of
  # window; the engine itself never touches them). The write at +50 ms
  # lands well inside both margins. `cannot-ignore` keeps unused_import's
  # ignore-comment offers out of the set, so the doc's single row is the
  # mechanical import removal and the byte-exact verdicts below hold. A
  # missed window (scheduling spike) answers `applied`, never a false
  # `stale` — up to 4 attempts absorb only jitter; a genuinely broken
  # drift correction still fails the leg.
  leg_begin fix-stale "a file changed under the fix run is refused as stale, not clobbered"
  make_pkg "$base/st" "st"
  wf "$base/st/analysis_options.yaml" 'analyzer:
  exclude:
    - lib/src
  cannot-ignore:
    - unused_import
linter:
  rules:
    - prefer_final_locals
'
  wf "$base/st/lib/helper.dart" 'int helper() => 1;
'
  wf "$base/st/lib/main.dart" "import 'helper.dart';

int main() => 2;
"
  gen_files "$base/st" 2500 stalefill 60
  local attempt=0 stale_rc=0 doc=""
  while [ "$attempt" -lt 4 ]; do
    attempt=$((attempt + 1))
    wf "$base/st/lib/main.dart" "import 'helper.dart';

int main() => 2;
"
    "$DARTQ" fix "$base/st/lib" \
      >"$EVIDENCE/fix-stale-$attempt.out" 2>"$EVIDENCE/fix-stale-$attempt.err" &
    local fix_pid=$!
    sleep 0.05
    # The concurrent edit mid-collect: the unused import's URI changes, so
    # the row's source slice survives nowhere in the apply-time text — the
    # anchor is gone, which is the `stale` verdict.
    wf "$base/st/lib/main.dart" "import 'renamed.dart';

int main() => 2;
"
    stale_rc=0
    wait "$fix_pid" || stale_rc=$?
    doc="$(cat "$EVIDENCE/fix-stale-$attempt.out")"
    case "$doc" in *outcome:stale*) break ;; esac
  done
  assert_eq "$stale_rc" 0 "a stale outcome is data, not a failed run"
  if [ "$attempt" -gt 1 ]; then
    ok "the concurrent edit landed inside the collect-to-apply window (attempt $attempt)"
  fi
  if grep -q 'outcome:stale' "$EVIDENCE/fix-stale-$attempt.out"; then
    ok "the lon document records the stale outcome"
  else
    bad "the lon document records the stale outcome" \
      "attempts=4 out=$(head -c 300 "$EVIDENCE/fix-stale-4.out") err=$(head -c 200 "$EVIDENCE/fix-stale-4.err")"
  fi
  assert_contains "$doc" "summary:{applied:0 stale:1}" \
    "the run applied nothing: the drifted row was refused whole"
  assert_eq "$(cat "$base/st/lib/main.dart")" "import 'renamed.dart';

int main() => 2;" \
    "the concurrent edit's bytes survive (not reverted, not clobbered)"

  # --- rename --write: the clean apply ---------------------------------
  leg_begin rename-write "rename --write rewrites both files and prints the record"
  make_pkg "$base/rn" "rn"
  # lib/a.dart:1 `int alpha() => 1;` — column 5 is the `a` of `alpha`.
  wf "$base/rn/lib/a.dart" 'int alpha() => 1;
'
  wf "$base/rn/lib/b.dart" "import 'a.dart';

int beta() => alpha();
"
  run_dartq_in rename-dry "$base/rn" rename "lib/a.dart:1:5" --new-name zulu --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "the dry run prints the edit set"
  assert_contains "$(cat "$(out_file rename-dry)")" 'new_name:zulu' \
    "the lon document carries the vocabulary edit set"
  assert_contains "$(cat "$base/rn/lib/a.dart")" "alpha" "the dry run wrote nothing"
  run_dartq_in rename-write "$base/rn" rename "lib/a.dart:1:5" --new-name zulu \
    --sdk "$base/sdk" --write
  assert_eq "$DQ_RC" 0 "the write run applies in place"
  assert_contains "$(cat "$base/rn/lib/a.dart")" "int zulu() => 1;" "a.dart was renamed"
  assert_contains "$(cat "$base/rn/lib/b.dart")" "zulu();" "b.dart's use was renamed"

  # --- rename: outside-workspace refusal --------------------------------
  # A path dependency: renaming `extra` at its use proposes edits in the
  # dependency's own file — outside the --dir workspace — which --write
  # must refuse before touching anything.
  leg_begin rename-refusal "rename refuses edits outside the workspace with a clear error"
  make_pkg "$base/pd/pg" "pg"
  make_pkg "$base/pd/dep" "dep"
  wf "$base/pd/dep/lib/extra.dart" 'int extra() => 1;
'
  python3 - "$base/pd/pg/.dart_tool/package_config.json" <<'PYEOF'
import json, sys
path = sys.argv[1]
config = json.load(open(path))
config["packages"].append(
    {"name": "dep", "rootUri": "../../dep", "packageUri": "lib/", "languageVersion": "3.5"})
open(path, "w").write(json.dumps(config))
PYEOF
  wf "$base/pd/pg/pubspec.yaml" 'name: pg
environment:
  sdk: ^3.5.0
dependencies:
  dep:
    path: ../dep
'
  # main.dart line 3 `int main() => extra();` — `e` of extra at column 15.
  wf "$base/pd/pg/lib/main.dart" "import 'package:dep/extra.dart';

int main() => extra();
"
  run_dartq_in rename-outside "$base/pd/pg" rename "lib/main.dart:3:15" --new-name zulu \
    --sdk "$base/sdk" --write
  assert_eq "$DQ_RC" 64 "the outside-workspace edit set is the usage tier"
  assert_contains "$(err_of rename-outside)" "refusing to write outside the workspace" \
    "the refusal names its reason"
  assert_contains "$(cat "$base/pd/pg/lib/main.dart")" "extra();" "the consumer file is untouched"
  assert_contains "$(cat "$base/pd/dep/lib/extra.dart")" "int extra() => 1;" \
    "the dependency file is untouched"
  unset DART_SDK
}
