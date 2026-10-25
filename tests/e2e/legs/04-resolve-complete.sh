# legs/04-resolve-complete.sh — the position faces one-shot: `dartq
# resolve` (DECL / REF / MISS machine rows) and `dartq complete` (PREFIX /
# CAND rows) at a symbol, at a boundary, and inside a string.
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

  leg_begin pos-resolve "resolve answers DECL/REF/MISS by cursor position"
  # The use of `alpha` in main.dart.
  run_dartq pos-ref resolve "lib/main.dart:3:15" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "resolve at a symbol exits clean"
  assert_contains "$(cat "$(out_file pos-ref)")" "REF|$base/p/lib/main.dart|" \
    "the REF row locates the queried file"
  assert_contains "$(cat "$(out_file pos-ref)")" "|alpha|" "the REF row names the symbol"
  assert_contains "$(cat "$(out_file pos-ref)")" "lib/alpha.dart" \
    "the REF row's target columns point at the declaration file"
  assert_fields "$(out_file pos-ref)" "REF|" 13 "REF rows carry the 13-field machine grammar"

  # The declaration of `alpha` in alpha.dart line 1 (column 5).
  run_dartq pos-decl resolve "lib/alpha.dart:1:5" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "resolve at the declaration exits clean"
  assert_contains "$(cat "$(out_file pos-decl)")" "DECL|" "the DECL row answers"
  assert_contains "$(cat "$(out_file pos-decl)")" "|alpha|" "the DECL row names the symbol"
  assert_fields "$(out_file pos-decl)" "DECL|" 7 "DECL rows carry the 7-field machine grammar"

  # A past-end line and a cursor inside a string both answer MISS.
  run_dartq pos-miss-line resolve "lib/main.dart:99:1" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "a past-end cursor is clean"
  assert_eq "$(cat "$(out_file pos-miss-line)")" "MISS" "a past-end cursor answers MISS"
  run_dartq pos-miss-str resolve "lib/main.dart:5:16" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "a cursor inside a string is clean"
  assert_eq "$(cat "$(out_file pos-miss-str)")" "MISS" "a cursor inside a string answers MISS"

  # --- completion ------------------------------------------------------
  leg_begin pos-complete "complete names the candidates under one cursor"
  # Cursor right after the `a` of `alpha` (column 16): prefix `a`.
  run_dartq pos-cand complete "lib/main.dart:3:16" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "complete at a symbol prefix exits clean"
  assert_contains "$(cat "$(out_file pos-cand)")" "PREFIX|a" "the PREFIX row spells the fragment"
  assert_contains "$(cat "$(out_file pos-cand)")" "CAND|alpha|" "alpha is a candidate"
  assert_not_contains "$(cat "$(out_file pos-cand)")" "CAND|label|" \
    "only prefix-matching names are candidates (label does not match `a`)"
  # A cursor inside the string literal completes with an empty prefix
  # (the char before the cursor is the quote, not an identifier char) —
  # never an error.
  run_dartq pos-cand-str complete "lib/main.dart:5:16" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 0 "complete inside a string is clean"
  assert_contains "$(cat "$(out_file pos-cand-str)")" "PREFIX|" \
    "the empty-prefix row still prints (the cursor follows no identifier)"

  # --- usage errors ------------------------------------------------------
  leg_begin pos-usage "malformed selectors are the usage tier"
  run_dartq pos-bad-sel resolve "lib/main.dart" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "a selector without a position is usage"
  assert_contains "$(err_of pos-bad-sel)" "PATH:LINE:COL" "the error spells the selector grammar"
  run_dartq pos-gone resolve "lib/gone.dart:1:1" "$base/p" --sdk "$base/sdk"
  assert_eq "$DQ_RC" 64 "a missing file is usage"
}
