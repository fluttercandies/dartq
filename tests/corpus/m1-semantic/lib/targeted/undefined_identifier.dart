// Targets undefined_identifier: an unresolved name is assigned at
// statement scope (frozen M1 undefined family; already fires on the syntax
// corpus, dedicated fixture pins per-code attribution).
void main() {
  x = 1;
  print(x);
}
