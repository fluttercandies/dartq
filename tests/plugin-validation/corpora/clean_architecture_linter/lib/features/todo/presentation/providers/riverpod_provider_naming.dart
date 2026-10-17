// Trigger for clean_architecture_linter `riverpod_provider_naming` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_provider_naming_v2_rule_test.dart):
// a `@riverpod` function whose provider name breaks the naming
// convention.
const riverpod = _Riverpod();

class _Riverpod {
  const _Riverpod();
}

class TodoRepository {}

@riverpod
TodoRepository todoRepo(ref) => TodoRepository();
