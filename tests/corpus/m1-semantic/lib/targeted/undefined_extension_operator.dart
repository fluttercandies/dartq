// Targets undefined_extension_operator: an explicit extension application is
// used as an operator receiver for an operator the extension does not
// declare.
extension E on int {
  int operator +(int other) => 1;
}

void f() {
  print(E(1) * 2);
}
