// messages-example: analyzer/messages.yaml StaticWarningCode.deadNullAwareExpression
int f(int x) {
  return x ?? 0;
}
