// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToFinal
class C {
  final v = 0;
}

void f(C c) {
  c.v = 1;
}
