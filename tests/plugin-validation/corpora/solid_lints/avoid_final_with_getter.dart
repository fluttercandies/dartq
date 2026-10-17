// Trigger for solid_lints `avoid_final_with_getter`.
// Shape: a `final` field paired with a redundant getter — the rule's
// flagged form.
class Box {
  final int _value = 0;

  int get value => _value;
}
