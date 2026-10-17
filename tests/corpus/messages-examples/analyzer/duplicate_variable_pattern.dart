// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicateVariablePattern
void f((int, int) r) {
  if (r case (var a, 0) && (0, var a)) {
    print(a);
  }
}
