// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToFinalLocal
void f() {
  final x = 0;
  x = 3;
  print(x);
}
