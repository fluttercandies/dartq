// From dart-sdk pkg/front_end/parser_testcases/error_recovery/await_not_in_async.dart (BSD-3-Clause, Copyright the Dart project authors)
// golden: strip these 2 header lines before scanning; offsets in .tokens.json are relative to the remainder
Future<void> f() => Future.value();

void g() {
  await f();
}
