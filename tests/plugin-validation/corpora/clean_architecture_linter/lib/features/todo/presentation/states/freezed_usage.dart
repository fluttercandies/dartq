// Trigger for clean_architecture_linter `freezed_usage` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/freezed_usage_v2_rule_test.dart):
// a presentation state class using `Equatable` instead of `freezed`.
// Stand-in: the upstream flags the `package:equatable` import URI and the
// `extends Equatable` clause; a local `Equatable` class keeps the file
// self-contained and resolution-free.
class TodoState extends Equatable {
  @override
  List<Object?> get props => [];
}

abstract class Equatable {
  const Equatable();

  List<Object?> get props;
}
