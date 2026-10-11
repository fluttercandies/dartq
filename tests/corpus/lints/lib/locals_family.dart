// locals and top-level variables family — fixture wave 2 kitchen file.
// One violation per rule, each on its own line.
// Expected rows: 7, all keyed locals_family.dart — AVOID_INIT_TO_NULL
// (`int? offset = null;` — the nullable-type face valid on both sides),
// PREFER_TYPING_UNINITIALIZED_VARIABLES (`var pending;`), 
// UNNECESSARY_NULLABLE_FOR_FINAL_VARIABLE_DECLARATIONS (`final int? width
// = 3;` local, non-null literal initializer — the local face the official
// test_nullableType_variable pins), UNNECESSARY_LATE (top-level `late int
// cached = 9;` — every variable initialized), 
// PREFER_FUNCTION_DECLARATIONS_OVER_VARIABLES (top-level `final handler`
// holding a function literal), NO_LEADING_UNDERSCORES_FOR_LOCAL_IDENTIFIERS
// (`var _tally`, not a bare wildcard), PROVIDE_DEPRECATION_MESSAGE
// (`@deprecated` without an argument list).
void offsetUser() {
  int? offset = null;
  print(offset);
}

void typingUser() {
  var pending;
  print(pending);
}

void widthUser() {
  final int? width = 3;
  print(width);
}

late int cached = 9;

final handler = (int input) {
  print(input);
};

void counterUser() {
  var _tally = 1;
  print(_tally);
}

@deprecated
void legacyEntry() {}
