// Trigger for clean_architecture_linter `dependency_inversion` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/domain_rules/dependency_inversion_v2_rule_test.dart):
// a usecase depending on a concrete data-layer implementation.
import '../../data/repositories/dependency_inversion_target.dart';

class GetTodoUseCase {
  final TodoRepositoryImpl repository;

  GetTodoUseCase(this.repository);
}
