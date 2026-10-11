// unrelated_type_equality_checks — audit r1 thin row. Scalar-literal
// operand pairs (the dartq ported face: syntax-knowable scalar literals;
// both sides judge int/String and int/bool unrelated).
// Expected rows: 2 — UNRELATED_TYPE_EQUALITY_CHECKS keyed
// unrelated_type_equality_checks.dart (`1 == 'a'`; `1 != true`).
void f() {
  if (1 == 'a') {
    print(0);
  }
  if (1 != true) {
    print(1);
  }
}
