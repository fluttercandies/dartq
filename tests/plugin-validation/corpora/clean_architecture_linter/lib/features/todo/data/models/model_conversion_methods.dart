// Trigger for clean_architecture_linter `model_conversion_methods` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/model_conversion_methods_v2_rule_test.dart):
// a freezed model missing the entity conversion surface.
const freezed = Object();

class Todo {}

@freezed
sealed class TodoModel {
  const factory TodoModel({
    required Todo entity,
    String? etag,
  }) = TodoModelImpl;
}

class TodoModelImpl implements TodoModel {
  const TodoModelImpl({required this.entity, this.etag});

  final Todo entity;
  final String? etag;
}
