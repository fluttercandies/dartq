// Targets wrong_number_of_parameters_for_operator: a binary operator is
// declared with two parameters where one is required.
class C {
  int operator +(int a, int b) => 1;
}
