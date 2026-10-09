// Targets type_parameter_on_operator: type parameters aren't allowed when
// defining an operator.
class C {
  int operator +<T>(int other) => 1;
}
