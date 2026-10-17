// Trigger for clean_architecture_linter
// `riverpod_extension_scoped_dependency` (v2). Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_extension_scoped_dependency_v2_rule_test.dart):
// a `@Dependencies` extension listing providers the host `@Riverpod`
// dependencies do not scope.
class Riverpod {
  const Riverpod({required List<Object> dependencies});
}

class Dependencies {
  const Dependencies(List<Object> deps);
}

const executeRoutine = Object();
const manageReminder = Object();
const toggleTodo = Object();

@Riverpod(dependencies: [toggleTodo])
class PomodoroNotifier {}
