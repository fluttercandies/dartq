// Targets unchecked_use_of_nullable_value: a nullable expression used as a
// condition. Oracle message (nail-pinned): "A nullable expression can't be
// used as a condition."
void f(bool? b) {
  if (b) {
    print(b);
  }
}
