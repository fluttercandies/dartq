// messages-example: analyzer/messages.yaml CompileTimeErrorCode.newWithNonType
int f() => 0;

void g() {
  new f();
}
