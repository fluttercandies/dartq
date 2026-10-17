// Trigger for leancode_lint `prefer_equatable_mixin`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/prefer_equatable_mixin_test.dart):
// a class that `extends Equatable` instead of mixing it in.
class MyState extends Equatable {
  @override
  List<Object?> get props => [];
}

abstract class Equatable {
  List<Object?> get props;
}
