// @dart = 2.9
// invalid_case_patterns — official rule faces
// (pkg/linter invalid_case_patterns.dart:41-88): a legacy `case` head
// whose expression is not a valid 3.0 constant-pattern spelling reports
// at the expression — a non-const list/set-or-map literal, a dart:core
// identical(...) call, a unary prefix whose operand is not an integer
// literal, any binary or conditional expression, a dart:core length
// property access, an is check, or the bare `_` identifier.
//
// The registration bails WITH the patterns feature (the lint exists for
// pre-3.0 libraries only), so this fixture pins the library to 2.9 —
// both sides register the rule and judge the legacy heads there.
// Expected rows: 1 — INVALID_CASE_PATTERNS keyed invalid_case_patterns.dart,
// message "This expression is not valid in a 'case' clause in Dart 3.0.":
//   the `1 + 2` binary expression
void f(dynamic o) {
  switch (o) {
    case 1 + 2:
      break;
    case 3:
      break;
  }
}
