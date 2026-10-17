// Trigger for dart_code_linter `prefer-commenting-analyzer-ignores`.
// Shape: an `// ignore:` comment with no explanatory text — the rule's
// flagged form.
void f() {
  // ignore: unused_local_variable
  final x = 1;
}
