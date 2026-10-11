// Targets unnecessary_type_check: an `is` test whose result is statically
// constant (_checkAllTypeChecks polarity pair).
void f(int x) {
  if (x is int) {
    print(1);
  }
}
