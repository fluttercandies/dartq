// Trigger for clean_architecture_linter `datasource_exception_types` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/data_rules/datasource_exception_types_v2_rule_test.dart):
// a datasource throwing a non-domain exception type.
class TodoRemoteDataSource {
  Future<void> fetch() {
    throw const FormatException('bad');
  }
}
