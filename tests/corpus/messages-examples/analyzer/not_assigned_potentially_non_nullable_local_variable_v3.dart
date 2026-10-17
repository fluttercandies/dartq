// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notAssignedPotentiallyNonNullableLocalVariable (variant 3)
int h(bool b) {
  int x;
  if (b) {
    x = 1;
  }
  if (b) {
    return x * 2;
  }
  return 0;
}
