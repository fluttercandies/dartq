// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedSetter
class C {
  int x = 0;
  void m(int y) {
    this.z = y;
  }
}
