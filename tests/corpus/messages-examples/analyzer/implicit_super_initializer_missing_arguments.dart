// messages-example: analyzer/messages.yaml CompileTimeErrorCode.implicitSuperInitializerMissingArguments
class A {
  A(int x);
}

class B extends A {
  B();
}
