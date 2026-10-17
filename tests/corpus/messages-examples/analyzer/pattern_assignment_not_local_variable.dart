// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternAssignmentNotLocalVariable
class C {
  var x = 0;

  void f((int, int) r) {
    (x, _) = r;
  }
}
