// Trigger for dart_code_linter `avoid-top-level-members-in-tests`.
// Shape: a non-`main` top-level member in a `*_test.dart` file — the
// rule's flagged form. The corpus harness feeds this file under a
// `*_test.dart`-shaped trigger; the member below is the flagged shape.
int helperValue = 0;

void main() {}
