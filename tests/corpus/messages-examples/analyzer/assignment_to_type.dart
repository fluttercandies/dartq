// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToType
class C {}

void f() {
  C = 0;
}
