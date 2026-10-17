// Trigger for clean_architecture_linter `repository_interface` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/domain_rules/repository_interface_v2_rule_test.dart):
// a concrete repository class in the domain layer must be abstract; the
// flagged form is the plain class implementing persistence concerns.
class TodoRepository {
  Future<Map<String, Object?>> findById(String id) => Future.value({});
}

class Todo {}
