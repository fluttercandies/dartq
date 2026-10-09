// Targets wrong_number_of_type_arguments_extension: an explicit extension
// application is given the wrong number of type arguments.
extension E<T> on int {
  int get g => 1;
}

void f() {
  print(E<int, String>(1).g);
}
