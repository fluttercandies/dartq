// messages-example: analyzer/messages.yaml CompileTimeErrorCode.missingVariablePattern
void f((int, int) r) {
  if (r case (var a, 0) || (0, _)) {
    print(a);
  }
}
