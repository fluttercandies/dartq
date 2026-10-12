// avoid_catches_without_on_clauses — official rule faces
// (pkg/linter avoid_catches_without_on_clauses.dart:30-104): a bare
// `catch` whose body neither rethrows nor rethrows the caught exception
// is reported; `on`-clauses, `rethrow;` bodies, and `throw e;` bodies
// are the official valid-use exemptions.
// Expected rows: 2 — AVOID_CATCHES_WITHOUT_ON_CLAUSES keyed
// avoid_catches_without_on_clauses.dart, one per bare catch:
//   `catch (e) { print(e); }`       -> row at the catch keyword
//   `catch (e, stack) { ... }`      -> row at the catch keyword
void f() {
  try {
    throw 1;
  } catch (e) {
    print(e);
  }
}

void g() {
  try {
    throw 2;
  } catch (e, stack) {
    print(stack);
  }
}

// GOOD: on-clause forms and the valid-use bodies.
void h() {
  try {
    throw 3;
  } on int catch (e) {
    print(e);
  }
}

void i() {
  try {
    throw 4;
  } catch (e) {
    rethrow;
  }
}

void j() {
  try {
    throw 5;
  } catch (e) {
    throw e;
  }
}
