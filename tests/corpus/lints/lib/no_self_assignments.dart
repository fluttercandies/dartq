// no_self_assignments — official rule faces
// (pkg/linter no_self_assignments.dart): an `=` assignment whose both
// sides are identifiers with the same name reports. The dartq face covers
// the bare-identifier form; compound assignments read the left operand
// and are excluded by the `=` gate.
// Expected rows: 2 — NO_SELF_ASSIGNMENTS keyed no_self_assignments.dart,
// message "The variable or property is being assigned to itself.":
//   the `a = a` assignment
//   the `b = b` assignment
void f(int a, int b) {
  a = a;
  b = b;

  // GOOD: distinct names and compound assignments pass.
  a = b;
  a += a;
}
