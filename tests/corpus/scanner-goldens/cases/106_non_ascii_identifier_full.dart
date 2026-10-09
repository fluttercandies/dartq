// From dart-sdk pkg/front_end/parser_testcases/error_recovery/comment_on_non_ascii_identifier.dart (BSD-3-Clause, Copyright the Dart project authors)
// golden: strip these 2 header lines before scanning; offsets in .tokens.json are relative to the remainder
main() {
  // Identifiers can't have non-ascii characters.
  // Having this comment shouldn't trigger any asserts though.
  int æFoo = 42;
  // Try comment on an identifier that doesn't start with a non-ascii char too.
  int fooÆ = 42;
  // Try comment on an OK identifier too.
  int foo = 42;
  print(/* comment */ æFoo);
  print(/* comment */ fooÆ);
  print(/* comment */ foo);
}