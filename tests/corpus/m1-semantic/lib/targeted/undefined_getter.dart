// Targets undefined_getter: a getter that the receiver type does not declare
// is read (frozen M1 undefined family, resolution-design-spec 4.2).
void main() {
  int x = 1;
  print(x.noSuchGetter);
}
