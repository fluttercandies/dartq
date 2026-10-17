// messages-example: analyzer/messages.yaml CompileTimeErrorCode.definitelyUnassignedLateLocalVariable
void f(bool b) {
  late int x;
  print(x);
}
