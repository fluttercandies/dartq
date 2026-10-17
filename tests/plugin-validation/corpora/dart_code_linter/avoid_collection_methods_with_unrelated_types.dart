// Trigger for dart_code_linter `avoid-collection-methods-with-unrelated-types`.
// Shape: a typed collection method called with an unrelated argument type —
// the rule's flagged form.
void main() {
  final map = Map<int, String>();
  map["str"] = "value";

  final list = [10, 20, 30];
  list.remove("str");
}
