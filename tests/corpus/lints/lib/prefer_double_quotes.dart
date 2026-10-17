// prefer_double_quotes — official rule faces
// (pkg/linter prefer_double_quotes.dart, the shared QuoteVisitor's
// useSingle: false face): a single-quoted literal (simple or
// interpolated, raw included) reports at the literal, unless the cooked
// value carries a `"`, a nested string sits inside one of its `${...}`
// groups, or the literal is itself nested inside an interpolation.
// Double-quoted spellings never report.
// Expected rows: 2 — PREFER_DOUBLE_QUOTES keyed prefer_double_quotes.dart,
// message "Unnecessary use of single quotes.":
//   the simple `'no quote'` literal
//   the interpolated `'no quote $x'` literal
var s = 'no quote';

var x = "other";
var t = 'no quote $x';

// GOOD: a `"` in the value, a nested string in an interpolation group,
// and a double-quoted spelling pass.
var g1 = 'has double quote "';
var g2 = 'foo ${x == 'other'} bar';
var g3 = "uses double";
