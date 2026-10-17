// use_test_throws_matchers — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/use_test_throws_matchers_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// The `fail` element must resolve to the declaring library URI
// package:test_api/src/frontend/expect.dart — provided by the corpus's
// vendored package:test_api (vendor/test_api, shaped exactly as the
// upstream test's setUp builds it).
// Inventory: 1 POSITIVE + 3 NEGATIVE = all 4 upstream tests, none skipped.
// Expected rows (oracle leg): 1 — USE_TEST_THROWS_MATCHERS keyed
// use_test_throws_matchers.dart at the `fail('fail');` statement.

// POSITIVE upstream test_failInTry: the try body ends in fail(...) with a
// single catch and no finally.
import 'package:test_api/test_api.dart';

void fFailInTry() {
  try {
    fFailInTry();
    fail('fail');
  } catch (e) {}
}

// NEGATIVE upstream test_failWithExpectInCatch: a finally block skips the
// rule.
void fFailWithExpectInCatch() {
  try {
    fFailWithExpectInCatch();
    fail('fail');
  } catch (e) {
    expect(e, null);
  } finally {
    print('hello');
  }
}

// NEGATIVE upstream test_failWithExpectInOnCatch: two catch clauses skip
// the rule.
void fFailWithExpectInOnCatch() {
  try {
    fFailWithExpectInOnCatch();
    fail('fail');
  } on Exception catch (e) {
    expect(e, null);
  } catch (e) {
    expect(e, null);
  }
}

// NEGATIVE upstream test_noFail: no fail in the body.
void fNoFail() {
  try {
    fNoFail();
  } catch (e) {}
}
