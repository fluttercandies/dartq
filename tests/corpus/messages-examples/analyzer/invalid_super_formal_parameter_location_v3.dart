// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidSuperFormalParameterLocation (variant 3)
class A {
  A(int x);
}

class D extends A {
  D() : super(0);

  void m(super.x) {}
}
