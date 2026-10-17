// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notAssignedPotentiallyNonNullableLocalVariable (variant 2)
int g(bool b) {
  int x;
  if (b) {
    x = 1;
  }
  return x * 2;
}
