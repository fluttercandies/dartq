// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicateRestElementInPattern
void f(List<int> x) {
  if (x case [0, ..., ...]) {}
}
