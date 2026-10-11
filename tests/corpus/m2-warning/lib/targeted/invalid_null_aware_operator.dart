// Targets invalid_null_aware_operator: ?. is applied to a non-nullable
// operand (the M1 frozen row scored in the WARNING tier).
void f(int x) {
  print(x?.isEven);
}
