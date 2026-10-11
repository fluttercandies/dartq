// Intentional diagnostics: the wrong_number_of_type_arguments family, one
// proven trigger shape per variant (dart 3.13.1 measured in the m1 gate:
// invocations report _element, property-access tear-offs report _function,
// plain type annotations report the base code). The annotation case lives
// on a top-level late variable: reading a late local before assignment is
// its own error, which would add an unintended code.
class Box<T> {
  static T pick<T>(T value) => value;

  M take<M>(M value) => value;
}

// expect: wrong_number_of_type_arguments
late Box<int, String> gOverloaded;

void open() {
  // expect: wrong_number_of_type_arguments_element
  print(Box.pick<int, String>(1));

  final handle = Box<int>().take;
  // expect: wrong_number_of_type_arguments_function
  var bound = handle<int, String>;
  print(bound);
}
