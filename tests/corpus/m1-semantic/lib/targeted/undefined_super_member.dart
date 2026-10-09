// Targets undefined_super_member: super resolves to a member the superclass
// does not declare (needs a subclass; frozen M1 undefined family).
class A {}

class B extends A {
  void f() {
    super.noSuchMember();
  }
}
