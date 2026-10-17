// Trigger for clean_architecture_linter `datasource_no_result_return` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/datasource_no_result_return_v2_rule_test.dart):
// a datasource method returning a bare model instead of a `Result`
// wrapper.
class TodoRemoteDataSource {
  Future<TodoModel> getTodo() async => TodoModel();
}

class Result<T, E> {}

class TodoModel {}
