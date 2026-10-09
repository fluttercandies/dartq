// Targets undefined_operator: a user-defined class with no operator
// declaration is used as an operator receiver (frozen M1 undefined family).
class C {}

void main() {
  print(C() + 1);
}
