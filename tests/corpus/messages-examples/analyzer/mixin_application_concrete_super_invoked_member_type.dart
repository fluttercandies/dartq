// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinApplicationConcreteSuperInvokedMemberType
class I {
  void foo([int? p]) {}
}

class A {
  void foo(int p) {}
}

abstract class B extends A implements I {
  @override
  void foo([int? p]);
}

mixin M on I {
  void bar() {
    super.foo(42);
  }
}

abstract class C extends B with M {}
