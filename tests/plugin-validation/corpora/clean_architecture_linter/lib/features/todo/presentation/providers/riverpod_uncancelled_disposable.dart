// Trigger for clean_architecture_linter `riverpod_uncancelled_disposable`
// (v2). Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_uncancelled_disposable_v2_rule_test.dart):
// a notifier leaving a watched disposable uncancelled.
const riverpod = _Riverpod();

class _Riverpod {
  const _Riverpod();
}

@riverpod
class PomodoroNotifier {
  Future<void> build() async {
    final timer = ref.watch(timerServiceProvider);
    timer.start(onComplete: _onComplete);
  }

  Ref get ref => Ref();

  void _onComplete() {}
}

class Ref {
  T watch<T>(T provider) => provider;
}

class TimerService {
  void start({required void Function() onComplete}) {}
}

final timerServiceProvider = TimerService();
