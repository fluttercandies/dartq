// document_ignores — official rule faces
// (pkg/linter document_ignores.dart:59-104): an `// ignore:` or
// `// ignore_for_file:` comment citing at least one diagnostic with no
// trailing free text reports unless the line above is itself a comment.
// An empty citation (`// ignore:`) has no elements and never reports.
// Expected rows: 2 — DOCUMENT_IGNORES keyed document_ignores.dart, message
// "Missing documentation explaining why the diagnostic is ignored.":
//   the undocumented `// ignore: unused_element` line
//   the undocumented `// ignore_for_file: unused_local_variable` line
int x = 0;

// ignore: unused_element
int _y = 0;

// ignore_for_file: unused_local_variable

// GOOD: a documented ignore (comment above, or trailing free text in the
// ignore itself) and an empty citation pass.
// Text above documents the ignore below.
// ignore: dead_code
int _z = 0;

// ignore: unused_field, I need this around
int _w = 0;

// ignore:
int _v = 0;
