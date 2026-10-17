// Trigger for leancode_lint `avoid_direct_collection_equality_checks`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_direct_collection_equality_checks_test.dart):
// a collection compared with `==`/`!=`.
bool test(List<int> a, List<int> b) {
  return a == b;
}
