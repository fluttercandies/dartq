// messages-example: analyzer/messages.yaml CompileTimeErrorCode.labelUndefined
void f() {
  for (int i = 0; i < 10; i++) {
    for (int j = 0; j < 10; j++) {
      if (j != 0) {
        break loop;
      }
    }
  }
}
