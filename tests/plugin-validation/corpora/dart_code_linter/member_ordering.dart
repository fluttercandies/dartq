// Trigger for dart_code_linter `member-ordering`.
// Shape: a public method declared before a public field — the rule's
// flagged ordering form.
class BadlyOrdered {
  void method() {}

  final field = 0;
}
