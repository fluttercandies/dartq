// Targets undefined_class: a type annotation names a class that is not in
// scope (frozen M1 undefined family, resolution-design-spec 4.2).
void main() {
  NoSuchClass x;
  print(x);
}
