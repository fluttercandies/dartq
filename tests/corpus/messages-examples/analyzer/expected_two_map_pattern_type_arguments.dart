// messages-example: analyzer/messages.yaml CompileTimeErrorCode.expectedTwoMapPatternTypeArguments
void f(Object x) {
  if (x case <int>{0: _}) {}
}
