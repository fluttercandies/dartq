// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notEnoughPositionalArgumentsSingular
void f(int a, int b) {}
void g() {
  f(0);
}
