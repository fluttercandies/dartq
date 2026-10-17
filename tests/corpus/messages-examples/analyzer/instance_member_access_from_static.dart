// messages-example: analyzer/messages.yaml CompileTimeErrorCode.instanceMemberAccessFromStatic
class C {
  int x = 0;

  static int m() {
    return x;
  }
}
