// messages-example: analyzer/messages.yaml CompileTimeErrorCode.recursiveConstructorRedirect (variant 2)
abstract class A {
  factory A() = B;
}

class B implements A {
  factory B() = A;
  B.named();
}
