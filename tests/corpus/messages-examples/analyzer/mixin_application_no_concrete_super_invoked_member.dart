// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinApplicationNoConcreteSuperInvokedMember
abstract class A {
  void m();
}

mixin M on A {
  void bar() {
    super.m();
  }
}

abstract class B extends A with M {}
