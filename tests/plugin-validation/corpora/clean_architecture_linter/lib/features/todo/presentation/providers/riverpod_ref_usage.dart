// Trigger for clean_architecture_linter `riverpod_ref_usage` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_ref_usage_v2_rule_test.dart):
// a notifier using `ref.read` inside `build`.
const riverpod = _Riverpod();

class _Riverpod {
  const _Riverpod();
}

@riverpod
class TodoNotifier {
  Object build() {
    return ref.read(currentUserProvider);
  }

  Ref get ref => Ref();
}

class Ref {
  T read<T>(T provider) => provider;
}

final currentUserProvider = Object();
