// @dart = 3.6
// require_trailing_commas — official rule faces
// (pkg/linter require_trailing_commas.dart:58-146): every multi-line
// argument, parameter, assert, and collection-literal group ends with a
// trailing comma. A trailing comma after the last element exits, so does
// a group whose delimiters share a line; among the exceptions only a
// MULTILINE last element qualifies (a block-bodied function literal, a
// string literal, an anonymous function call whose parens share a line,
// or a collection literal).
//
// Registration is version-gated: the rule registers only below the 3.7
// tall-style milestone, so this fixture pins the library to 3.6 — both
// sides register the rule there.
// Expected rows: 2 — REQUIRE_TRAILING_COMMAS keyed
// require_trailing_commas.dart, message "Missing a required trailing comma.":
//   the `]` of the multiline list literal
//   the `)` of the multiline argument list whose last argument is a
//   single-line string
void g(int a, String s) {}

void f() {
  var l = [
    1,
    2
  ];
  g(
    1,
    'two'
  );
}

// GOOD: a trailing comma exits, and a multiline string last argument is
// exempt.
void h(bool a) {
  assert(
    a,
    'msg',
  );
  g(
    1,
    '''
hi
''',
  );
}
