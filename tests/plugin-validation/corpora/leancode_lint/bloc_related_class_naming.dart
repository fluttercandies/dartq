// Trigger for leancode_lint `bloc_related_class_naming`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/bloc_related_class_naming_test.dart):
// a `Bloc`'s event/state classes that don't follow the `<Bloc>Event`/
// `<Bloc>State` naming convention.
class WrongEvent {}

class WrongState {}

class MyBloc extends Bloc<WrongEvent, WrongState> {
  MyBloc() : super(WrongState());
}

abstract class Bloc<Event, State> {
  Bloc(this.state);
  final State state;
}
