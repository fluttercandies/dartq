// Trigger for clean_architecture_linter `exception_naming_convention` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/domain_rules/exception_naming_convention_v2_rule_test.dart):
// a domain exception type named `DataException` instead of the package's
// `<Feature>Exception` convention.
class DataException implements Exception {}
