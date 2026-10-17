// Trigger for clean_architecture_linter `repository_no_throw` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/repository_no_throw_v2_rule_test.dart):
// a repository implementation method that throws.
abstract class TodoRepository {}

class TodoRepositoryImpl implements TodoRepository {
  Future<void> save() async {
    throw Exception('failed');
  }
}
