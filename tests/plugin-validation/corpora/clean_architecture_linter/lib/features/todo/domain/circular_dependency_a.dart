// Trigger for clean_architecture_linter `circular_dependency` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/cross_layer/circular_dependency_rule_test.dart):
// a file importing a cycle back to itself.
import 'circular_dependency_b.dart';

class CircularA {}
