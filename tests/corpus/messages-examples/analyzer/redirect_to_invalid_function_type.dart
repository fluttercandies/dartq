// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToInvalidFunctionType
abstract class A {
  factory A() = B;
}

class B implements A {
  B(int x);
  B.zero();
}
