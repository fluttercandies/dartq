// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToInvalidReturnType
class A {}

class B implements C {}

class C {
  factory C() = A;
}
