// Targets the explicit-extension-application undefined family in one file:
// undefined_extension_getter, undefined_extension_method and
// undefined_extension_setter (explicit application is required; the
// implicit-application paths report the plain undefined_* codes instead,
// see undefined_extension_getter.dart). Extension dispatch cannot reach
// Object members, so the method name needs no noSuchMethod avoidance.
// Trigger conditions: the pinned resolution trigger table (lookup failures against the resolved import scope).
// Hand-crafted from the code semantics; no official test file.
extension X on String {
  int get g => 1;
}

void use() {
  print(X('a').noSuchGetter);
  X('a').noSuchMemberHere();
  X('a').noSuchSetter = 1;
}
