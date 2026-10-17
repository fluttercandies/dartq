// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superFormalParameterWithoutAssociatedPositional
class A {
  A({int? x});
}

class B extends A {
  B(super.x);
}
