// simplify_variable_pattern — official rule faces
// (pkg/linter simplify_variable_pattern.dart:41-118): a pattern field
// with an explicit name whose variable pattern declares the same name
// reports at the field name with the member kind spelled into the
// message (`isEven: var isEven` — the getter identification). The member
// kind resolves over the containing pattern: a same-file class chain
// answers method, getter, or field; every unresolvable type answers
// getter.
// Expected rows: 1 — SIMPLIFY_VARIABLE_PATTERN keyed
// simplify_variable_pattern.dart, message
// "The getter identification 'isEven:' is redundant and can be removed.":
//   the `isEven:` field whose sub-pattern declares `isEven`
void f(Object o) {
  if (o case int(isEven: var isEven)) {}

  // GOOD: distinct names keep the identification.
  if (o case int(isEven: var flag)) {}
}
