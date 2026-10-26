# legs/04-resolve-complete.sh — the position faces one-shot: `dartq
# resolve` and `dartq complete` at a symbol, at a boundary, and inside a
# string (the answer envelope's hit verdicts, read on the json face; the
# constant lon default gets its own shape check).
#
# Cursor arithmetic (1-based columns): lib/main.dart line 3 is
# `int main() => alpha();` — the `a` of `alpha` sits at column 15; line 5
# is `const label = 'alpha';` — the opening quote is column 15, so column
# 16 is the first position inside the string.

legs_04_run() {
  local base="$E2E_WORK/pos"
  mkdir -p "$base"
  make_stub_sdk "$base/sdk"
  make_pkg "$base/p" "pos"
  wf "$base/p/lib/alpha.dart" 'int alpha() => 1;
'
  wf "$base/p/lib/main.dart" "import 'alpha.dart';

int main() => alpha();

const label = 'alpha';
"

  leg_begin pos-resolve "resolve answers the hit verdicts by cursor position"
  # The default lon face: one canonical document, the vocabulary envelope.
  run_dartq pos-ref-lon resolve "lib/main.dart:3:15" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "resolve at a symbol exits clean"
  assert_contains "$(cat "$(out_file pos-ref-lon)")" "v:1" \
    "the lon document carries the vocabulary envelope"
  assert_contains "$(cat "$(out_file pos-ref-lon)")" "alpha" \
    "the lon document names the symbol"

  # The use of `alpha` in main.dart, structured on the json face.
  run_dartq pos-ref resolve "lib/main.dart:3:15" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "resolve at a symbol exits clean"
  json_check "$(out_file pos-ref)" \
    "j['hit']['kind']=='reference' and j['hit']['name']=='alpha' and j['file'].endswith('lib/main.dart')" \
    "the reference hit locates the queried file and names the symbol"
  json_check "$(out_file pos-ref)" "j['hit']['file'].endswith('lib/alpha.dart')" \
    "the hit's target columns point at the declaration file"
  json_check "$(out_file pos-ref)" \
    "j['hit']['decl_kind']=='function' and j['hit']['range']['off']==36" \
    "the hit carries the relation and the name span"

  # The declaration of `alpha` in alpha.dart line 1 (column 5).
  run_dartq pos-decl resolve "lib/alpha.dart:1:5" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "resolve at the declaration exits clean"
  json_check "$(out_file pos-decl)" \
    "j['hit']['kind']=='declaration' and j['hit']['name']=='alpha' and j['hit']['decl_kind']=='function'" \
    "the declaration hit answers"

  # A past-end line and a cursor inside a string both answer the miss.
  run_dartq pos-miss-line resolve "lib/main.dart:99:1" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "a past-end cursor is clean"
  json_check "$(out_file pos-miss-line)" "j['hit'] is None" "a past-end cursor answers the miss"
  run_dartq pos-miss-str resolve "lib/main.dart:5:16" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "a cursor inside a string is clean"
  json_check "$(out_file pos-miss-str)" "j['hit'] is None" "a cursor inside a string answers the miss"

  # --- completion ------------------------------------------------------
  leg_begin pos-complete "complete names the candidates under one cursor"
  # Cursor right after the `a` of `alpha` (column 16): prefix `a`.
  run_dartq pos-cand complete "lib/main.dart:3:16" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "complete at a symbol prefix exits clean"
  json_check "$(out_file pos-cand)" "j['prefix']=='a'" "the prefix field spells the fragment"
  json_check "$(out_file pos-cand)" \
    "any(i['name']=='alpha' for i in j['items'])" "alpha is a candidate"
  json_check "$(out_file pos-cand)" \
    "all(i['name'].startswith('a') for i in j['items'])" \
    "only prefix-matching names are candidates (label does not match `a`)"
  # A cursor inside the string literal completes with an empty prefix
  # (the char before the cursor is the quote, not an identifier char) —
  # never an error.
  run_dartq pos-cand-str complete "lib/main.dart:5:16" "$base/p" --sdk "$base/sdk" --format json
  assert_eq "$DQ_RC" 0 "complete inside a string is clean"
  json_check "$(out_file pos-cand-str)" "j['prefix']==''" \
    "the empty prefix still answers (the cursor follows no identifier)"

  # --- usage errors ------------------------------------------------------
  leg_begin pos-usage "malformed selectors are the usage tier"
  run_dartq pos-bad-sel resolve "lib/main.dart" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "a selector without a position is usage"
  assert_contains "$(err_of pos-bad-sel)" "PATH:LINE:COL" "the error spells the selector grammar"
  run_dartq pos-gone resolve "lib/gone.dart:1:1" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "a missing file is usage"
}
