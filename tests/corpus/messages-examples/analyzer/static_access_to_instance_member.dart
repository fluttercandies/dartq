// messages-example: analyzer/messages.yaml CompileTimeErrorCode.staticAccessToInstanceMember
class C {
  static int a = 0;

  int b = 0;
}

int f() => C.b;
