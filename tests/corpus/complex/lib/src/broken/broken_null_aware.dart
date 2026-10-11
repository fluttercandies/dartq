// Intentional diagnostics: invalid_null_aware_operator. `?.` applied to
// non-nullable operands; the dead null-aware is reported per occurrence.
void scan(int code, String label) {
  // expect: invalid_null_aware_operator
  print(code?.isEven);

  // expect: invalid_null_aware_operator
  print(label?.length);
}
