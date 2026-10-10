// Targets the accessor family (analyzer ParserErrorCode, pinned snapshot
// syntax-codes.tsv; the analyzer sources are not vendored in .third).
// Targets missing_get (TSV:207): a '=>' member without 'get'.
class MissingGet {
  int x => 1;
}

// Targets static_getter_without_body (TSV:271).
class StaticGetterWithoutBody {
  static int get x;
}

// Targets static_setter_without_body (TSV:273).
class StaticSetterWithoutBody {
  static set x(int v);
}

// Targets getter_in_function (TSV:145).
void getterInFunction() {
  int get g => 1;
}

// Targets setter_in_function (TSV:268).
void setterInFunction() {
  set s(int v) {}
}
