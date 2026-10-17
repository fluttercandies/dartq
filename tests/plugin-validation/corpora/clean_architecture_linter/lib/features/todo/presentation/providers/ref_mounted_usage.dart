// Trigger for clean_architecture_linter `ref_mounted_usage` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/ref_mounted_usage_v2_rule_test.dart):
// a `ref.mounted` check guarded the wrong way.
class Ref {
  bool get mounted => true;
}

void update(Ref ref) {
  if (!ref.mounted) {
    return;
  }
}
