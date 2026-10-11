// prefer_contains — audit r1 thin row; all three official message
// variants on distinct lines, typed-parameter receiver (the dartq pin
// face: `void f(List<int> xs)`), so the opaque-receiver over-report face
// from the divergence ledger is out of shape.
// Expected rows: 3 — PREFER_CONTAINS keyed prefer_contains.dart:
//   `indexOf == -1`  -> use-contains variant
//   `indexOf < -1`   -> always-false variant
//   `indexOf >= -1`  -> always-true variant
void f(List<int> xs) {
  if (xs.indexOf(1) == -1) {
    print(0);
  }
  if (xs.indexOf(1) < -1) {
    print(1);
  }
  if (xs.indexOf(1) >= -1) {
    print(2);
  }
}
