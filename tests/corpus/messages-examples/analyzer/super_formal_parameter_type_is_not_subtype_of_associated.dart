// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superFormalParameterTypeIsNotSubtypeOfAssociated
class A {
  A(num x);
}

class B extends A {
  B(String super.x);
}
