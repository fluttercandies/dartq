// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notAssignedPotentiallyNonNullableLocalVariable
String f() {
  int x;
  return x.toString();
}
