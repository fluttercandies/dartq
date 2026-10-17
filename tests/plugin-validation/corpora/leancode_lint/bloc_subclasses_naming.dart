// Trigger for leancode_lint `bloc_subclasses_naming`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/bloc_subclasses_naming_test.dart):
// a subclass of a bloc state/event whose name breaks the `<Base><Suffix>`
// convention.
class MyState {}

class WrongName extends MyState {}

class MyEvent {}

class MyBloc extends Bloc<MyEvent, MyState> {
  MyBloc() : super(MyState());
}

abstract class Bloc<Event, State> {
  Bloc(this.state);
  final State state;
}
