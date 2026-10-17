// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableAssignmentInsideGuard
void f(int x) {
  if (x case var a when (a = 1) > 0) {
    print(a);
  }
}
