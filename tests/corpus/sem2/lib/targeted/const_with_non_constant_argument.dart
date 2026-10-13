// Targets const_with_non_constant_argument: a constant creation whose
// argument is not a constant expression (the non-const variable 'n').
class C {
  final int x;
  const C(this.x);
}

var n = 1;

const z = C(n);
