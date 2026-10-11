// Boundary catch-pairs: the two catch codes are mutually exclusive by
// construction. The oracle auto-marks the exception parameter used whenever
// a stack parameter is declared OR the catch is bare (no `on` clause) -
// unused_local_elements_verifier.dart:73 visitCatchClause:
// `if (stackTraceParameter != null || node.onKeyword == null) addElement(e)`.
// So unused_catch_clause only fires with an `on` clause, no stack parameter,
// and an unused exception; unused_catch_stack only fires when a declared
// stack parameter is unused. A bare `catch (e) {}` reports nothing
// (unused_catch_clause_test.dart:54 test_unusedException).
void f() {
  try {
  } catch (e, s) {
    print(e); // warns unused_catch_stack only
  }
  try {
  } on FormatException catch (e) {
    print(1); // warns unused_catch_clause only (unused_catch_clause_test.dart:19 test_on_unusedException)
  }
  try {
  } catch (e, s) {} // warns unused_catch_stack only (bare catch auto-uses e; unused_catch_stack_test.dart:40 test_unusedStack)
}
