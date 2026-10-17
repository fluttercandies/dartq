// messages-example: analyzer/messages.yaml CompileTimeErrorCode.emptyMapPattern
void f(Map<int, String> x) {
  if (x case {}) {}
}
