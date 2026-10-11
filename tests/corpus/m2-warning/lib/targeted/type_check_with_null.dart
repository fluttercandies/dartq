// Targets type_check_with_null: an `is Null` test; null checks belong to
// '==' (best_practices_verifier's type-check family).
void f(int? x) {
  if (x is Null) {
    print(1);
  }
}
