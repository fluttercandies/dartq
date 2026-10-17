// messages-example: analyzer/messages.yaml CompileTimeErrorCode.useOfVoidResult
void f() {}

void g() {
  f().toString();
}
