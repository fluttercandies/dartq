// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToInvalidFunctionType (variant 2)
abstract class A {
  factory A(int x, {int y}) = B;
}

class B implements A {
  B(int x);
}
