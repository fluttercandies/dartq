// messages-example: analyzer/messages.yaml CompileTimeErrorCode.labelInOuterScope
void f() {
  loop:
  while (true) {
    void g() {
      break loop;
    }

    g();
  }
}
