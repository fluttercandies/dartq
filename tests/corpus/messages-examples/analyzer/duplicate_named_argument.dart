// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicateNamedArgument
void f(C c) {
  c.m(a: 0, a: 1);
}

class C {
  void m({int? a, int? b}) {}
}
