// Targets external_constructor_with_field_initializers: an external
// constructor cannot initialize fields via 'this.x' parameters.
class C {
  int x;
  external C(this.x);
}
