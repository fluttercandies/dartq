// Trigger for leancode_lint `avoid_catch_error`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_catch_error_test.dart):
// `Future.catchError` invoked on a future.
Future<void> test(Future<void> future) {
  return future.catchError((Object err, StackTrace st) {});
}
