// Targets wrong_number_of_type_arguments_method: a generic method
// instantiation with a type argument count that differs from the
// declaration. DRIFT (goal #125, dart 3.13.1 measured): the _method variant
// has no reachable emission path -- invocations report _element and
// property-access tear-offs report _function. The method tear-off construct
// is kept so the file starts emitting the target code if a future SDK
// restores the path; its current output
// (WRONG_NUMBER_OF_TYPE_ARGUMENTS_FUNCTION) stays in-set for the gate.
class C {
  T m<T>(T x) => x;
}

void main() {
  var h = C().m<int, String>;
  print(h);
}
