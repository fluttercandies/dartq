// prefer_asserts_with_message — official rule faces
// (pkg/linter prefer_asserts_with_message.dart): an assert statement or
// constructor assert initializer without a message reports at the assert.
// Expected rows: 2 — PREFER_ASSERTS_WITH_MESSAGE keyed
// prefer_asserts_with_message.dart, message
// "Missing a message in an assert.":
//   the `assert(b)` statement
//   the `assert(b)` initializer in `C`
void f(bool b) {
  assert(b);
}

class C {
  C(bool b) : assert(b);
}

// GOOD: an assert with a message passes in both positions.
void g(bool b) {
  assert(b, 'must hold');
}

class D {
  D(bool b) : assert(b, 'must hold');
}
