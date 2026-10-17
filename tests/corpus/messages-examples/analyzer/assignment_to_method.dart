// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assignmentToMethod
class C {
  void f() {}

  void g() {
    f = null;
  }
}
