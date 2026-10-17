// use_raw_strings — official rule faces
// (pkg/linter use_raw_strings.dart:29-56): a non-raw simple string whose
// every escape is `\\` or `\$` reports at the literal — those are exactly
// the escapes a raw string preserves; any other escape bails. An
// interpolated literal is not a simple string and never enters.
// Expected rows: 1 — USE_RAW_STRINGS keyed use_raw_strings.dart, message
// "Use a raw string to avoid using escapes.":
//   `'\\'` — the only escape is a doubled backslash
var f = '\\';

// GOOD: any other escape keeps the normal string, and a raw string is
// not this face.
var g1 = '\$ and \\ and \n';
var g2 = r'\\';
