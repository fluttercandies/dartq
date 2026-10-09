// Targets undefined_enum_constant: an enum constant that the enum does not
// declare is referenced through the enum type.
enum E { a }

void main() {
  print(E.b);
}
