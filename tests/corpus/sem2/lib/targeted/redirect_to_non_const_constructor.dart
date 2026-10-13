// Targets redirect_to_non_const_constructor: a constant redirecting
// factory whose target constructor is not const. Oracle message
// (nail-pinned): "A constant redirecting constructor can't redirect to a
// non-constant constructor."
class A {
  A();
}

class B {
  const factory B() = A;
}
