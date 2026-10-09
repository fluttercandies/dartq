// Targets external_constructor_with_initializer: an external constructor
// cannot have any initializers.
class C {
  int x;
  external C() : x = 1;
}
