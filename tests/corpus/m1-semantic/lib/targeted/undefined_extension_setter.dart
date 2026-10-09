// Targets undefined_extension_setter: an explicit extension application
// assigns a setter the extension does not declare.
extension E on String {
  set s(int value) {}
}

void f() {
  E('x').t = 1;
}
