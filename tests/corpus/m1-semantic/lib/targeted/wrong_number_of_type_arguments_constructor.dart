// Targets wrong_number_of_type_arguments_constructor: a constructor
// reference carries an explicit type argument list whose length differs
// from the class declaration. DRIFT (goal #125, dart 3.13.1 measured): no
// constructor-reference shape reaches the _constructor emission path --
// invocation, const invocation, tear-off and .new tear-off all report the
// plain variant instead. The construct is kept so the file starts emitting
// the target code if a future SDK restores the path; its current output
// (WRONG_NUMBER_OF_TYPE_ARGUMENTS) stays in-set for the gate.
class C<T> {
  C.named();
}

void main() {
  var c = C<int, String>.named;
  print(c);
}
