// Trigger for dart_code_linter `avoid-throw-in-catch-block`.
// Shape: a `throw` inside a `catch` block — the rule's flagged form.
void f() {
  try {
    risky();
  } catch (e) {
    throw e;
  }
}

void risky() {}
