// Targets wrong_number_of_type_arguments_element: an invocation is given an
// explicit type argument list whose length differs from the callee's
// declaration. On dart 3.13.1 the _element variant owns every invocation
// shape (top-level/static/method invocation, dot-shorthand, super and
// implicit-this tear-offs); the static-method invocation here is the
// measured canonical shape.
class C<T> {
  static T sm<T>(T x) => x;
}

void main() {
  print(C.sm<int, String>(1));
}
