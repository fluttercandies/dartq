// messages-example: analyzer/messages.yaml CompileTimeErrorCode.inconsistentInheritance
class A {
  void m({int? a}) {}
}

class B {
  void m({int? b}) {}
}

class C extends A implements B {}
