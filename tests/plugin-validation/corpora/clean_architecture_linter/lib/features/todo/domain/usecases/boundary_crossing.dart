// Trigger for clean_architecture_linter `boundary_crossing` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/cross_layer/boundary_crossing_rule_test.dart):
// a domain usecase importing a concrete data-layer implementation.
import '../../data/repositories/boundary_crossing_target.dart';

class GetTodoUseCase {}
