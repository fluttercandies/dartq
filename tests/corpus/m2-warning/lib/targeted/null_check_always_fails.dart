// Targets null_check_always_fails: a null assertion on an expression that
// always evaluates to null always throws (visitNullAssertionExpression).
void f() {
  print(null!);
}
