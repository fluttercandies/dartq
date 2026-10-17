// Trigger for leancode_lint `avoid_build_context_in_blocs`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/avoid_build_context_in_blocs_test.dart),
// de-templatized: the mocked `package:bloc` and `package:flutter` imports are
// replaced by inline stand-ins so the file is standalone.
class CounterEvent {
  CounterEvent([this.data]);
  final Object? data;
}

class CounterBloc extends Bloc<CounterEvent, int> {
  CounterBloc() : super(0);

  void add(CounterEvent event, BuildContext context) {
    // A BuildContext parameter on a Bloc surface leaks UI layer identity
    // into state management.
  }
}

// Inline stand-ins for the packages the upstream suite mocks.
abstract class Bloc<Event, State> {
  Bloc(this.state);
  final State state;
}

class BuildContext {}
