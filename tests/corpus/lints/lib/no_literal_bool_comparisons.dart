// no_literal_bool_comparisons — official rule faces
// (pkg/linter no_literal_bool_comparisons.dart:44-62): a boolean literal
// on either side of ==/!=/|/||/&/&&/^ whose OTHER operand is a
// non-nullable bool reports at the literal. The same-file written faces
// (variables, parameters, fields, `bool Function` returns) drive the
// type read; a written `bool?` stays silent like the official
// non-nullability gate.
// Expected rows: 7 — NO_LITERAL_BOOL_COMPARISONS keyed
// no_literal_bool_comparisons.dart, message
// "Unnecessary comparison to a boolean literal.":
//   `true` in `value && true` -> local variable operand
//   `true` in `true && value` -> literal on the left
//   `true` in `b == true` -> parameter operand
//   `false` in `flag != false` -> top-level variable operand
//   `true` in `true == true` -> both sides are literals
//   `true` in `box.flag || true` -> same-file class field operand
// plus UNRELATED_TYPE_EQUALITY_CHECKS keyed the same file (`==` in
// `someInt == true` — int parameter vs bool literal are unrelated).
bool flag = true;

class Box {
  bool flag = true;
}

void f(bool b, bool? maybe, dynamic dyn, int someInt) {
  bool value = true;
  Box box = Box();
  var r1 = value && true;
  var r2 = true && value;
  var r3 = b == true;
  var r4 = flag != false;
  var r5 = true == true;
  var r6 = box.flag || true;

  // GOOD: nullable/dynamic/non-bool operands, non-comparison operators
  // and literal-free comparisons stay silent. `g4` reports under
  // unrelated_type_equality_checks (int vs bool), not this rule.
  var g1 = maybe ?? true;
  var g2 = dyn == true;
  var g3 = value || maybe;
  var g4 = someInt == true;
  var g5 = value == flag;
}
