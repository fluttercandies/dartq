// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToAbstractClassConstructor
class A {
  factory A() = B;
}

abstract class B implements A {}
