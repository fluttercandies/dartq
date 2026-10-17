// Trigger for clean_architecture_linter `model_entity_direct_access` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/model_entity_direct_access_v2_rule_test.dart):
// a data-layer class reaching a model's `entity` directly.
class TodoModel {
  Todo get entity => Todo();
}

class Todo {}

class TodoRepositoryImpl {
  Todo convert(TodoModel model) {
    return model.entity;
  }
}
