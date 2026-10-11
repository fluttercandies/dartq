// The import target of the parent package's depend_on_referenced_packages
// fixture (lib/imports_family.dart imports this library undeclared — the
// nested package is resolvable through the corpus root package config but
// absent from the parent's dependency set, the rule's canonical face).
// Conforming: contributes no rows of its own.
void childHelper() {
  print(5);
}
