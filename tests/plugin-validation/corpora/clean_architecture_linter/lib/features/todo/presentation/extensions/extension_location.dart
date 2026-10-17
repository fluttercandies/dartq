// Trigger for clean_architecture_linter `extension_location` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/extension_location_v2_rule_test.dart):
// a UI-facing extension declared outside the presentation layer's
// extension location.
class Todo {}

extension TodoUIX on Todo {
  String get label => 'todo';
}
