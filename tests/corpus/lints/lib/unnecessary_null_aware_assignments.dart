// unnecessary_null_aware_assignments — audit r1 blind-spot row (0 fixtures).
// Expected rows: 1 — UNNECESSARY_NULL_AWARE_ASSIGNMENTS keyed
// unnecessary_null_aware_assignments.dart (`x ??= null`; simple identifier
// write, not a property/index assignment).
void f(int? x) {
  x ??= null;
}
