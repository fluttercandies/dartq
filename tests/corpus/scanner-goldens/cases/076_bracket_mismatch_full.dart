// From dart-sdk pkg/front_end/parser_testcases/error_recovery/bracket_mismatch_00.dart (BSD-3-Clause, Copyright the Dart project authors)
// golden: strip these 2 header lines before scanning; offsets in .tokens.json are relative to the remainder
void good() {
  [
    ([
    ])
  ];
}

void bad() {
  [
    (
    ])
  ];
}
