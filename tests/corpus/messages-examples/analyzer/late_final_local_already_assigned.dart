// messages-example: analyzer/messages.yaml CompileTimeErrorCode.lateFinalLocalAlreadyAssigned
int f() {
  late final int v;
  v = 0;
  v += 1;
  return v;
}
