// Targets unnecessary_null_comparison: a == null over an operand that cannot
// be null is constant (_checkForInvariantNullComparison).
void f(int x) {
  if (x == null) {
    print(1);
  }
}
