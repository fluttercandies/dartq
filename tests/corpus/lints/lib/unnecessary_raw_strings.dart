// unnecessary_raw_strings — official rule faces
// (pkg/linter unnecessary_raw_strings.dart:29-34): a raw simple string
// whose lexeme carries neither `\` nor `$` reports at the literal. A
// raw string never interpolates, so the raw head lexeme is the whole
// literal; an interpolated literal's middle segment can still open with
// an `r` and must stay silent.
// Expected rows: 1 — UNNECESSARY_RAW_STRINGS keyed
// unnecessary_raw_strings.dart, message
// "Unnecessary use of a raw string.":
//   the plain raw literal `r'a b c d'`
var s = r'a b c d';

// GOOD: a raw string carrying a backslash, and a non-raw spelling, pass.
var g1 = r'a b c\d';
var g2 = 'a b c d';
