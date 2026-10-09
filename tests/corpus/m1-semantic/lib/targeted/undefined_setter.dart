// Targets undefined_setter: a setter that the receiver type does not declare
// is assigned (frozen M1 undefined family, resolution-design-spec 4.2).
void main() {
  int x = 1;
  x.noSuchSetter = 2;
  print(x);
}
