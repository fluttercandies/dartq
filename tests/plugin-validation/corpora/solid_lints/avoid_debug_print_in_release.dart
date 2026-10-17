// Trigger for solid_lints `avoid_debug_print_in_release`.
// Shape: `debugPrint` called in release-reachable code — the rule's
// flagged form.
void f() {
  debugPrint('hello');
}

void debugPrint(String? message, {int? wrapWidth}) {}
