// Targets override_on_non_overriding_member: @override on a member no
// superinterface declares (annotation-driven override check).
class A {
  void f() {}
}

class B extends A {
  @override
  void g() {}
}
