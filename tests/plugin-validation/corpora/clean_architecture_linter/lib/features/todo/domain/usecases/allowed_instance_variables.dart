// Trigger for clean_architecture_linter `allowed_instance_variables` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/cross_layer/allowed_instance_variables_rule_test.dart):
// a usecase holding a forbidden instance-variable type (a data-layer
// datasource).
class TodoRemoteDataSource {}

class GetTodoUseCase {
  final TodoRemoteDataSource dataSource;

  const GetTodoUseCase(this.dataSource);
}
