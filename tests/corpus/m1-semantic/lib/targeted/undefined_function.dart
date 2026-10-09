// Targets undefined_function: an unresolved name is invoked as a function at
// top-level scope (frozen M1 undefined family, resolution-design-spec 4.2).
void main() {
  noSuchFunction();
}
