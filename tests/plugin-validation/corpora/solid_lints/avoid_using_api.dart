// Trigger for solid_lints `avoid_using_api`.
// Shape: usage of a banned API/declaration — the rule's flagged form.
void f() {
  BadClass().call();
}

class BadClass {
  void call() {}
}
