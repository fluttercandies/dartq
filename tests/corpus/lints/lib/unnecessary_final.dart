// unnecessary_final — official rule faces
// (pkg/linter unnecessary_final.dart): a `final` on a local variable
// declaration, a formal parameter, a for-each loop variable, or a
// declared variable pattern reports at the keyword. Fields, for-parts
// rows, and unmarked parameters are not the face.
// Expected rows: 2 — UNNECESSARY_FINAL keyed unnecessary_final.dart,
// message "Local variables should not be marked as 'final'.":
//   the `final` of the typed local `x`
//   the `final` of the untyped local `y`
void f() {
  final int x = 1;
  print(x);
  final y = 2;
  print(y);
}

// GOOD: a field is the official's unvisited face.
class A {
  final int z = 0;
}
