// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonGenerativeConstructor
class A {
  factory A.one() = B;
  A.two();
}

class B extends A {
  B() : super.one();
}
