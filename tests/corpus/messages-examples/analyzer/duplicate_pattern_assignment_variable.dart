// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicatePatternAssignmentVariable
int f((int, int) r) {
  int a;
  (a, a) = r;
  return a;
}
