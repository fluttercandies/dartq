// messages-example: analyzer/messages.yaml CompileTimeErrorCode.missingRequiredArgument
void f(int start, {required int end}) {}
void g() {
  f(3);
}
