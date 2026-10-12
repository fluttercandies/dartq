// leading_newlines_in_multiline_strings — official rule faces
// (pkg/linter leading_newlines_in_multiline_strings.dart:61-72): a
// multiline string spanning a line boundary whose first content character
// is not a newline reports at the literal. Interpolation reads its first
// segment; the raw prefix shifts the content offset by one.
// Expected rows: 2 — LEADING_NEWLINES_IN_MULTILINE_STRINGS keyed
// leading_newlines_in_multiline_strings.dart, message
// "Missing a newline at the beginning of a multiline string.":
//   `'''this...` -> plain multiline without a leading newline
//   `'''more $x` -> interpolated multiline without a leading newline
var x = 0;

var a = '''this
 is a multiline string''';

var b = '''more $x
than one line''';

// GOOD: a leading newline, a single-line multiline spelling and a raw
// literal with a leading newline comply.
var g1 = '''
starts with a newline''';
var g2 = '''single-line multiline spelling''';
var g3 = r'''
raw with a leading newline''';
