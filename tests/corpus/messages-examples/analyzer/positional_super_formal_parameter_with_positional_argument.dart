// messages-example: analyzer/messages.yaml CompileTimeErrorCode.positionalSuperFormalParameterWithPositionalArgument
class A {
  A(int x, int y);
}

class B extends A {
  B(int x, super.y) : super(x);
}
