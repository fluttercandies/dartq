// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constWithNonConst
class A {
  A();
}

A f() => const A();
