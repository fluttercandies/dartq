// avoid_returning_null_for_void — audit r1 blind-spot row (0 fixtures).
// Expected rows: 1 — AVOID_RETURNING_NULL_FOR_VOID keyed
// avoid_returning_null_for_void.dart (`return null;` out of a void
// function, the rule's canonical variant).
void f() {
  return null;
}
