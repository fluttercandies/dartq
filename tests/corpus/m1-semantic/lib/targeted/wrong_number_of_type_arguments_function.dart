// Targets wrong_number_of_type_arguments_function: an explicit
// instantiation of a function-typed target without invocation. On
// dart 3.13.1 the _function variant fires on property-access tear-offs
// (instance/static method tear-off, .call reference); simple-identifier and
// local-function tear-offs report the _element variant instead.
class C<T> {
  static T sm<T>(T x) => x;
}

void main() {
  var g = C.sm<int, String>;
  print(g);
}
