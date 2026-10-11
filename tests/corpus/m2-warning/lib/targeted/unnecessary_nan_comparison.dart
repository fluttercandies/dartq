// Targets unnecessary_nan_comparison: a comparison against 'double.nan' is
// constant (isDoubleNan).
void f(double x) {
  if (x == double.nan) {
    print(1);
  }
  if (x != double.nan) {
    print(2);
  }
}
