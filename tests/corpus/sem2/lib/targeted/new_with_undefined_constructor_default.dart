// Targets new_with_undefined_constructor_default: a named constructor
// invoked on a class that declares no constructors (only the implicit
// default exists).
class A {}

void f() {
  var a = A.named();
  print(a);
}
