// library_names — official rule faces
// (pkg/linter library_names.dart:28-46): a named `library` directive
// whose name is not lowercase_with_underscores reports at the whole name;
// a dotted name (`library one.Two.three;`) reports once across every
// segment — the validator faces the whole dotted text. A unit carries at
// most one library directive, so this file carries the title-case row;
// the dotted and compliant faces are the same validator.
// Expected rows: 1 — LIBRARY_NAMES keyed library_names.dart, message
// "The library name 'Foo' isn't a lower_case_with_underscores identifier.":
//   the name `Foo`
library Foo;

// GOOD: a compliant name and an unnamed `library;` pass (one directive
// per unit, so the compliant faces live in this comment).
//   library foo_bar;
//   library;
