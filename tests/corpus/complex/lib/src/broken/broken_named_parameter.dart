// Intentional diagnostics: undefined_named_parameter and undefined_function.
// The callee takes an optional positional parameter, so the unknown named
// argument is the only diagnostic on its line (a required positional would
// cascade a missing-positional code); the second call targets a top-level
// function that does not exist.
void weigh([int grams = 0]) {}

void intake() {
  // expect: undefined_named_parameter
  weigh(kilograms: 2);

  // expect: undefined_function
  transmitManifest();
}
