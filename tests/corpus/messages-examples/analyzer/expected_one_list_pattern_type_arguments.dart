// messages-example: analyzer/messages.yaml CompileTimeErrorCode.expectedOneListPatternTypeArguments
void f(Object x) {
  if (x case <int, int>[0]) {}
}
