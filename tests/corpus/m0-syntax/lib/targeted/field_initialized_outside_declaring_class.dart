// Targets field_initialized_outside_declaring_class: a field can only be
// initialized in its declaring class (super.field in an initializer list).
class A {}

class C extends A {
  C() : super.toString = 1;
}
