// prefer_if_elements_to_conditional_expressions — official rule faces
// (pkg/linter prefer_if_elements_to_conditional_expressions.dart:44-135):
// a conditional expression used directly as a collection element reports
// — an element of a list or set literal, the then/else branch of an
// if-element, or the body of a for-element. The report climbs the
// parenthesized wrappers; a spread or null-aware marker owns its
// expression and keeps the rule silent, as does a map entry key.
// Expected rows: 2 — PREFER_IF_ELEMENTS_TO_CONDITIONAL_EXPRESSIONS keyed
// prefer_if_elements_to_conditional_expressions.dart, message
// "Use an 'if' element to conditionally add elements.":
//   the `b ? 'c' : 'd'` list element
//   the `((b ? 'c' : 'd'))` set element, reported once over the parens
List<String> f(bool b) {
  return ['a', b ? 'c' : 'd', 'e'];
}

Set<String> h(bool b) {
  return {'a', ((b ? 'c' : 'd'))};
}

// GOOD: a map entry key and a conditional under a spread pass.
Map<String, int> m(bool b) {
  return {'a': 1, b ? 'c' : 'd' : 2};
}

List<String> s(bool b) {
  return ['a', ...(b ? ['c'] : ['d'])];
}
