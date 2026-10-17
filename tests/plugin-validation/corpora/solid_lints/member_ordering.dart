// Trigger for solid_lints `member_ordering`.
// Shape: a public method declared before a public field — the rule's
// flagged ordering form.
class BadlyOrdered {
  void method() {}

  final field = 0;
}
