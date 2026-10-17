// messages-example: analyzer/messages.yaml CompileTimeErrorCode.readPotentiallyUnassignedFinal
int f() {
  final int x;
  return x;
}
