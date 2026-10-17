// Trigger for solid_lints `avoid_similar_names`.
// Shape: two declarations with near-identical names — the rule's flagged
// form.
void f() {
  final value = 1;
  final values = 2;
}
