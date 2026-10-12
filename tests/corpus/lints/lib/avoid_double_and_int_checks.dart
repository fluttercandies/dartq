// avoid_double_and_int_checks — official rule faces
// (pkg/linter avoid_double_and_int_checks.dart:28-59): `x is double`
// as an if condition with an `else if (x is int)` on the same operand
// is reported (the second check can never hold once the first failed
// on the JS runtime); different operands and other orders are exempt.
// Expected rows: 1 — AVOID_DOUBLE_AND_INT_CHECKS keyed
// avoid_double_and_int_checks.dart:
//   `f: is double / else if is int` -> row at the second `is` keyword
void f(num p) {
  if (p is double) {
    print(1);
  } else if (p is int) {
    print(2);
  }
}

// GOOD: different operands break the pair; a lone check has no
// counterpart; `is!` forms and other orderings are different faces.
void g(num p, num q) {
  if (p is double) {
    print(3);
  } else if (q is int) {
    print(4);
  }
}

void h(num p) {
  if (p is double) {
    print(5);
  }
}

void i(num p) {
  if (p is double) {
    print(6);
  }
  if (p is int) {
    print(7);
  }
}
