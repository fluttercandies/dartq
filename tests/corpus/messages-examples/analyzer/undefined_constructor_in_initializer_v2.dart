// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedConstructorInInitializer (variant 2)
class A {
  A.n();
}

class B extends A {
  B() : super.m();
}
