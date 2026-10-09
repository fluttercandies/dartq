// Targets redirecting_constructor_with_body: redirecting constructors
// cannot have a body.
class C {
  C() : this.other() {}

  C.other();
}
