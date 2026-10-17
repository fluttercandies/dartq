// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedConstructorInInitializer
class A {
  A.n();
}

class B extends A {
  B() : super();
}
