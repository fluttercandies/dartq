// Targets unnecessary_non_null_assertion: a null assertion on an operand
// that cannot be null has no effect (visitNullAssertionExpression).
void f(int x) {
  print(x!);
}
