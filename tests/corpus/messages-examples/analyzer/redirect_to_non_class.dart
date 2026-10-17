// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToNonClass
C f() => throw 0;

class C {
  factory C() = f;
}
