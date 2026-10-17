// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonGenerativeImplicitConstructor
class A {
  factory A() => throw 0;

  A.named();
}

class B extends A {}
