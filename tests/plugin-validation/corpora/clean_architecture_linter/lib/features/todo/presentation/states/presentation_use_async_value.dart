// Trigger for clean_architecture_linter `presentation_use_async_value`
// (v2). Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/presentation_use_async_value_v2_rule_test.dart):
// a state class carrying `errorMessage`/`isLoading` fields instead of an
// `AsyncValue`.
const freezed = _Freezed();

class _Freezed {
  const _Freezed();
}

@freezed
class TodoState {
  const TodoState({this.errorMessage, this.isLoading = false});

  final String? errorMessage;
  final bool isLoading;
}
