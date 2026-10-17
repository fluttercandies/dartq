// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extraPositionalArguments
void f(int a, int b) {}
void g() {
  f(1, 2, 3);
}
