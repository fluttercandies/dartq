// combinators_ordering — official rule faces
// (pkg/linter combinators_ordering.dart:35-50): a show/hide combinator
// whose names are not in case-sensitive code-unit order reports at the
// combinator.
// Expected rows: 2 — COMBINATORS_ORDERING keyed
// combinators_ordering.dart, message
// "Sort combinator names alphabetically.":
//   `show min, max` -> unsorted show
//   `hide zeta, alpha` -> unsorted hide
import 'dart:math' show min, max;
export 'dart:math' hide zeta, alpha;

// GOOD: sorted combinators stay silent (case-sensitive order: `B` < `a`).
import 'dart:async' show Completer, Stream;
import 'dart:convert' hide Base64, ascii;
