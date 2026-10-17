// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superFormalParameterWithoutAssociatedPositional (variant 2)
class A {
  A(int x);
}

class B extends A {
  B(super.x, super.y);
}
