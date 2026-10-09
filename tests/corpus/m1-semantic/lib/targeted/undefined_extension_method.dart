// Targets undefined_extension_method: an explicit extension application
// invokes a method the extension does not declare.
extension E on String {
  void m() {}
}

void f() {
  E('x').n();
}
