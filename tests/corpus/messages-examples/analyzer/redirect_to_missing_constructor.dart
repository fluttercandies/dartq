// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToMissingConstructor
class A {
  factory A() = B.name;
}

class B implements A {
  B();
}
