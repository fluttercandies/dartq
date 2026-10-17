// Trigger for leancode_lint `missing_equatable_props`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/missing_equatable_props_test.dart):
// an `Equatable` mixin class whose `props` omits declared fields.
class MyState with Equatable {
  MyState(this.a, this.b);

  final int a;
  final String b;

  @override
  List<Object?> get props => [a];
}

mixin Equatable {
  List<Object?> get props;
}
