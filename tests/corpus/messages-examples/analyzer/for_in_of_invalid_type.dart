// messages-example: analyzer/messages.yaml CompileTimeErrorCode.forInOfInvalidType
void f(Map<String, String> m) {
  for (String s in m) {
    print(s);
  }
}
