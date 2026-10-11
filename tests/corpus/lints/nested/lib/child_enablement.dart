// Child-set enablement face: unnecessary_type_check is enabled ONLY via
// nested/analysis_options.yaml. The rule left the SDK linter in the Dart
// 3.15 main re-pin, so the oracle reports undefined_lint on the options
// file and never lints this unit, while dartq keeps it as a
// Group::Diagnostic opt-in and fires here — both rows sit outside the
// frozen universe codes file (gate-invisible by design); the fixture
// proves the child enable set reaches the dartq lint stage end to end.
// Expected rows: oracle 1 non-lint (the 3.13.1 oracle still ships
// unnecessary_type_check as an analyzer STATIC_WARNING, so `dart
// analyze` emits it as a non-lint diagnostic on the `if (42 is int)`
// face — outside the 88-code gate set either way); dartq 1 —
// UNNECESSARY_TYPE_CHECK keyed nested/lib/child_enablement.dart.
void childEnablement() {
  if (42 is int) {
    print(7);
  }
}
