// Targets dead_null_aware_expression: a null-aware ?? over an operand that
// cannot be null is dead (flow analysis).
void f(int x, int y) {
  print(x ?? y);
}
