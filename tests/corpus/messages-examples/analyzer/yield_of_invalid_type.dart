// messages-example: analyzer/messages.yaml CompileTimeErrorCode.yieldOfInvalidType
Iterable<int> get zero sync* {
  yield '0';
}
