// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedNamedParameter
class C {
  void m({int? b}) {}
}

void f(C c) {
  c.m(a: 1);
}
