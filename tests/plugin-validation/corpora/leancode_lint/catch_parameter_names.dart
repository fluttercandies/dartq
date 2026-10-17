// Trigger for leancode_lint `catch_parameter_names`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/catch_parameter_names_test.dart):
// catch clauses whose exception/stack-trace parameter names violate the
// naming convention.
Object? someFunction() {
  try {} catch (exception) {
    return exception;
  }

  try {} catch (err, stackTrace) {
    return stackTrace;
  }

  try {} on Exception catch (err, st) {
    return st;
  }

  try {} catch (err, st) {
    return st;
  }

  return null;
}
