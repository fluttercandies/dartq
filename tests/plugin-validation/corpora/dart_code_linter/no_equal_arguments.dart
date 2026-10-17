// Trigger for dart_code_linter `no-equal-arguments`.
// Shape: two positional arguments passed the same expression — the rule's
// flagged form.
void f() {
  contains(text, text);
}

void contains(String a, String b) {}

final text = 'value';
