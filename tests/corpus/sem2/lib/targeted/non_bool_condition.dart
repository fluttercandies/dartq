// Targets non_bool_condition: an if condition whose static type is not
// bool. Oracle message (nail-pinned): "Conditions must have a static type
// of 'bool'."
void f(int i) {
  if (i) {
    print(i);
  }
}
