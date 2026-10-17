// Trigger for clean_architecture_linter `datasource_abstraction` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/datasource_abstraction_v2_rule_test.dart):
// a data-layer datasource not hidden behind an abstraction.
class TodoRemoteDataSource {
  Future<Map<String, Object?>> fetch() => Future.value({});
}
