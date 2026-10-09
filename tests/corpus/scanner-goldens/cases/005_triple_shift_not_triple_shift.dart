// From dart-sdk pkg/front_end/parser_testcases/no-triple-shift/triple_shift_not_triple_shift.dart (BSD-3-Clause, Copyright the Dart project authors)
// golden: strip these 2 header lines before scanning; offsets in .tokens.json are relative to the remainder
// @dart=2.12
// From https://github.com/dart-lang/sdk/issues/46886
extension on Symbol {
  String operator >(_) => "Greater Than used";
  String call(_) => "Called";
}

void main() {
  print(#>>>(2));
}

abstract class Foo extends List<List<List<String>>> {}
