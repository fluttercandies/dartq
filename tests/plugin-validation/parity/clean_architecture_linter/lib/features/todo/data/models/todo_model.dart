// Trigger for clean_architecture_linter `model_field_duplication` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/model_field_duplication_v2_rule_test.dart):
// a data-layer model mirroring the entity's fields one for one.
class TodoModel {
  final String id;
  final String title;
  final bool isCompleted;

  const TodoModel({
    required this.id,
    required this.title,
    required this.isCompleted,
  });
}

class Todo {
  final String id;
  final String title;
  final bool isCompleted;

  const Todo({
    required this.id,
    required this.title,
    required this.isCompleted,
  });
}
