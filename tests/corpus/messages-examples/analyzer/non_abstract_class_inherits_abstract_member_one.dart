// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonAbstractClassInheritsAbstractMemberOne
abstract class A {
  void m();
}

class B extends A {}
