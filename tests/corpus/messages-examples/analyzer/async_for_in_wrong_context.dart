// messages-example: analyzer/messages.yaml CompileTimeErrorCode.asyncForInWrongContext
void f(Stream<int> list) {
  await for (var e in list) {
    print(e);
  }
}
