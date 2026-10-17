// Trigger for solid_lints `avoid_unnecessary_type_assertions`.
// Shape: an `is` assertion on an expression already of that type — the
// rule's flagged form.
void f(String value) {
  final check = value is String;
}
