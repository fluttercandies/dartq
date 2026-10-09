// Targets undefined_method: a method that the receiver type does not declare
// is invoked (frozen M1 undefined family, resolution-design-spec 4.2). The
// name must not collide with Object.noSuchMethod.
void main() {
  int x = 1;
  x.noSuchMethodHere();
}
