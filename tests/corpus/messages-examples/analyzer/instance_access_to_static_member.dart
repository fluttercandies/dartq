// messages-example: analyzer/messages.yaml CompileTimeErrorCode.instanceAccessToStaticMember
void f(C c) {
  c.zero;
}

class C {
  static int zero = 0;
}
