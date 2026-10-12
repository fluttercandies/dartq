// Targets top-level scope resolution in one file: undefined_function (an
// unresolved simple identifier invoked as a function) and
// undefined_identifier (an unresolved simple identifier read). The
// single-trigger shapes live in undefined_function.dart and
// undefined_identifier.dart. Trigger conditions: the pinned
// resolution trigger table (lookup failures against the resolved import scope). Hand-crafted from the code semantics; no
// official test file.
void main() {
  noSuchTopLevel();
  print(noSuchVariable);
}
