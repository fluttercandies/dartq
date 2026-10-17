// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superFormalParameterWithoutAssociatedNamed
class A {
  A({int? x});
}

class B extends A {
  B({super.y});
}
