// Trigger for dart_code_linter `avoid-redundant-async`.
// Shape: an `async` function whose body needs no `await` — the rule's
// flagged form.
Future<int> fastestBranch(Future<int> left, Future<int> right) async {
  return Future.any([left, right]);
}
