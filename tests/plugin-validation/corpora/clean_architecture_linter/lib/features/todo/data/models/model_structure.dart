// Trigger for clean_architecture_linter `model_structure` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/model_structure_v2_rule_test.dart):
// a model holding the entity as a field instead of exposing the required
// structure.
class Todo {}

class TodoModel {
  const TodoModel({required this.entity});

  final Todo entity;
}
