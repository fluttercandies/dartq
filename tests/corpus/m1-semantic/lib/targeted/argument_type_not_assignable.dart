// Targets argument_type_not_assignable: a String argument is passed where
// int is expected (static nullability four, types-nullability-spec 3.2).
void f(int x) {}

void main() {
  f('not an int');
}
