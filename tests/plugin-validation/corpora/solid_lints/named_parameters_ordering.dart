// Trigger for solid_lints `named_parameters_ordering`.
// Shape: a call whose named arguments are out of the configured
// declaration order — the rule's flagged form.
void example({required String first, required String second}) {}

void f() {
  example(second: 'b', first: 'a');
}
