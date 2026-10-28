// directives_ordering — official rule faces
// (pkg/linter directives_ordering.dart:120-280): dart: URIs go before
// other directives of their kind, package: URIs go before relative ones
// after the first absolute run, exports come after all imports, and each
// dart:/relative/package section is neighbor-sorted alphabetically. A
// directive violating several checks reports exactly once.
// Expected rows: 2 — DIRECTIVES_ORDERING keyed directives_ordering.dart,
// messages "Place 'dart:' imports before other imports." and
// "Place 'package:' imports before relative imports.":
//   `import 'dart:html'` after the relative import
//   `import 'package:meta/meta.dart'` after the relative import
import 'dart:math';
import 'imports_family_target.dart';
import 'dart:html';
import 'package:meta/meta.dart';

// GOOD: a fully sorted multi-section unit passes the walk.
//   import 'dart:aaa';
//   import 'package:aaa/aaa.dart';
//   import 'aaa/aaa.dart';
