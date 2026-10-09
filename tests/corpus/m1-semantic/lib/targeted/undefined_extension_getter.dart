// Targets undefined_extension_getter: an explicit extension application
// reads a getter the extension does not declare (needs the extension in
// scope; implicit application would report undefined_getter instead).
extension E on String {
  int get g => 1;
}

void f() {
  print(E('x').h);
}
