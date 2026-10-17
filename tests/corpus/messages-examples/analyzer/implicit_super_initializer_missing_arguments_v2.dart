// messages-example: analyzer/messages.yaml CompileTimeErrorCode.implicitSuperInitializerMissingArguments (variant 2)
class A {
  A({required int x});
}

class B extends A {
  B();
}
