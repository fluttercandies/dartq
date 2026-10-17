// prefer_single_quotes — official rule faces
// (pkg/linter prefer_single_quotes.dart:73-106): a double-quoted literal
// (simple or interpolated, raw included) reports at the literal, unless
// the cooked value carries a single quote, a nested string sits inside
// one of its `${...}` groups, or the literal is itself nested inside an
// interpolation. Single-quoted spellings never report.
// Expected rows: 2 — PREFER_SINGLE_QUOTES keyed prefer_single_quotes.dart,
// message "Unnecessary use of double quotes.":
//   the simple `"no quote"` literal
//   the interpolated `"no quote $p"` literal
var x = "no quote";

void f(String p) {
  "no quote $p";

  // GOOD: a `'` in the value, a nested string in an interpolation group,
  // and a single-quoted spelling pass.
  var g1 = "has quote '";
  var g2 = "foo ${p == 'x'} bar";
  var g3 = 'uses single';
}
