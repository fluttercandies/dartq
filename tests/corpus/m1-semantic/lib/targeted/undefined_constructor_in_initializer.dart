// Targets undefined_constructor_in_initializer: the initializer list invokes
// a named super constructor that the superclass does not declare.
class A {}

class B extends A {
  B() : super.named();
}
