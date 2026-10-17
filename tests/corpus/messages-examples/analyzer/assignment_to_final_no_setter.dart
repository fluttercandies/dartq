// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToFinalNoSetter
class C {
  int get x => 0;
  set y(int p) {}
}

void f(C c) {
  c.x = 1;
}
