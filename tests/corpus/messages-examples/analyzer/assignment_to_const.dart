// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToConst
const c = 0;

void f() {
  c = 1;
  print(c);
}
