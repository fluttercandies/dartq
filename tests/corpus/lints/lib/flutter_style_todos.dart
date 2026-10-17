// flutter_style_todos — official rule faces
// (pkg/linter flutter_style_todos.dart:24-84): a comment whose lexeme
// starts with the trigger face (`//+`, whitespace, `TODO` case-
// insensitive) but not the expected face (`//`, whitespace, `TODO`,
// `(username)`, `: `) reports at the whole comment. Both regexes anchor
// at index 0: a TODO after other text never fires, `/// TODO(...)`
// reports (the expected `//` cannot cross the third slash), and block
// comments never trigger.
// Expected rows: 2 — FLUTTER_STYLE_TODOS keyed flutter_style_todos.dart,
// message "To-do comment doesn't follow the Flutter style.":
//   the `// TODO something` line (no username, no colon)
//   the `/// TODO(user): bla` doc-comment line
// TODO something
var x = 1;

/// TODO(user): bla
var y = 2;

// GOOD: the proper format and a non-anchored TODO pass.
// TODO(username): bla
var z = 3;

// comment TODO(user): bla
var w = 4;
