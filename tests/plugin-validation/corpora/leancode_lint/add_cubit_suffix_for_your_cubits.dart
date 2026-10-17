// Trigger for leancode_lint `add_cubit_suffix_for_your_cubits`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/add_cubit_suffix_for_cubits_test.dart):
// a `Cubit` subclass whose name lacks the `Cubit` suffix.
class NotProperlyNamed extends Cubit<bool> {
  NotProperlyNamed() : super(true);
}

abstract class Cubit<State> {
  Cubit(this.state);
  final State state;
}
