// lines_longer_than_80_chars — official rule faces
// (pkg/linter lines_longer_than_80_chars.dart:139-186): every line longer
// than 80 code units reports from column 81 to the line end, unless
// exempted — a multiline string covers all its lines, a single-line
// string or comment whose content contains `/` or `\` covers its own
// line, and an 80-unit line passes.
// Expected rows: 2 — LINES_LONGER_THAN_80_CHARS keyed
// lines_longer_than_80_chars.dart, message
// "The line length exceeds the 80-character limit.":
//   the 88-unit string line (81 spaces inside the literal)
//   the 84-unit plain code line below it
    '                                                                                 ';
    '                                                           ';

var q = 1; // this trailing comment is long, but every content line of a comment needs a slash or backslash to be covered, so it reports

// GOOD: a URI-valued string covers its own long line, and a multiline
// string covers all its lines.
var u =
    '/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

var m = """
This line is a long, very long, very very long, very very very long,
and this second line inside the multiline literal is also far too long
""";
