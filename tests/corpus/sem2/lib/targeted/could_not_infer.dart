// Targets could_not_infer: type inference fails for the generic
// constructor invocation's type parameter. Verbatim mirror of the
// regression-nail source (oracle 3.13.1 pins could_not_infer at the 'C'
// position and argument_type_not_assignable, m1, at the closure); the nail
// also records dartq emitting nothing there, which is exactly the gap the
// sem2 tier makes visible.
class C<X> {
  C(void Function(X) x);
}

T check<T>(C<List<T>> f) {
  print('check<$T>(...)');
  return 42 as T;
}

void main() {
  var x =
      check(
        C((List<int> x) {}),
      );
  print(x);
}
