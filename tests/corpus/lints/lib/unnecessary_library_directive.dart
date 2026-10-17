// unnecessary_library_directive — official rule faces
// (pkg/linter unnecessary_library_directive.dart:38-50): a `library`
// directive with no documentation comment and no annotations reports at
// the directive, unless the unit carries a `part` directive. A plain `//`
// comment does not exempt; a `///` doc comment or `/** */` block doc does.
// Expected rows: 1 — UNNECESSARY_LIBRARY_DIRECTIVE keyed
// unnecessary_library_directive.dart, message
// "Library directives without comments or annotations should be avoided.":
//   the bare `library lib;` directive
library lib;

class A {}

// GOOD: a doc-commented directive is exempt.
//   /// Docs.
//   library with_comment;
