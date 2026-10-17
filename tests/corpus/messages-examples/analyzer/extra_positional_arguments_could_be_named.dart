// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extraPositionalArgumentsCouldBeNamed
void f(int a, int b, {int? c}) {}
void g() {
  f(1, 2, 3);
}
