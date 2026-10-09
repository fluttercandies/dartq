// Targets invalid_null_aware_operator: ?. is applied to a non-nullable
// operand (static nullability four, types-nullability-spec 3.4). WARNING
// severity by design; still authored per the frozen-list decision.
void f(int x) {
  print(x?.isEven);
}
