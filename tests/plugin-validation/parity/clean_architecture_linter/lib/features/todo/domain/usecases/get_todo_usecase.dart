// Trigger for clean_architecture_linter `usecase_no_result_return` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/domain_rules/usecase_no_result_return_v2_rule_test.dart):
// the suite's flagged usecase returns a value without wrapping it in the
// package's Result contract.
class GetTodoUseCase {
  Todo call() => Todo();
}

class Result<T, E> {}

class Todo {}
