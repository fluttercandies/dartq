// messages-example: analyzer/messages.yaml CompileTimeErrorCode.forInOfInvalidElementType
void f() {
  for (int e in <String>[]) {
    print(e);
  }
}
