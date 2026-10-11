// Targets unnecessary_null_check_pattern: a null-check pattern over a
// non-nullable matched type has no effect (shared_type_analyzer).
void f(int n) {
  switch (n) {
    case _?:
      break;
  }
}
