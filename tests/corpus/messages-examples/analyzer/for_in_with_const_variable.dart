// messages-example: analyzer/messages.yaml CompileTimeErrorCode.forInWithConstVariable
void f() {
  for (const x in [0, 1, 2]) {
    print(x);
  }
}
