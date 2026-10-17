// avoid_bool_literals_in_conditional_expressions — official rule faces
// (pkg/linter avoid_bool_literals_in_conditional_expressions.dart:33-45):
// a conditional expression whose arms are both bool-typed reports once
// per bool-literal arm, at the whole conditional. The same-file written
// type face (a bare-identifier non-literal arm) drives the non-literal
// side; a nullable written `bool?` arm stays silent.
// Expected rows: 2 — AVOID_BOOL_LITERALS_IN_CONDITIONAL_EXPRESSIONS keyed
// avoid_bool_literals_in_conditional_expressions.dart, message
// "Conditional expressions with a 'bool' literal can be simplified.":
//   the `true` then-arm of `b ? true : false`
//   the `false` else-arm of `b ? true : false`
void f(bool b) {
  var x = b ? true : false;

  // GOOD: non-bool arms and a nullable written arm stay silent.
  var g1 = b ? 1 : 2;
  bool? maybe;
  var g2 = b ? maybe : false;
}
