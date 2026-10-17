// Trigger for clean_architecture_linter `repository_pass_through` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/repository_pass_through_v2_rule_test.dart):
// a data-layer repository implementation whose methods hand entities back
// unmodified instead of mapping through models.
class TodoRepositoryImpl implements TodoRepository {
  @override
  Todo getTodo() => Todo();
}

abstract class TodoRepository {
  Todo getTodo();
}

class Todo {}
