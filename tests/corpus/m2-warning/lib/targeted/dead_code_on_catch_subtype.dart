// Targets dead_code_on_catch_subtype: an on-clause whose exception type is a
// subtype of an earlier clause's is dead (_CatchClausesVerifier).
class A {}

class B extends A {}

void f() {
  try {
    throw B();
  } on B {
    print(1);
  } on A {
    print(2);
  }
}
