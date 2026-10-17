// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToFunction
void f() {}

void g() {
  f = () {};
}
