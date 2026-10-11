// Targets unnecessary_null_assert_pattern: a null-assert pattern over a
// non-nullable matched type has no effect (shared_type_analyzer).
void f(int n) {
  switch (n) {
    case _!:
      break;
  }
}
