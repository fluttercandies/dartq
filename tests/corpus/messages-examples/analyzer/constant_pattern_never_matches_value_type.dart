// messages-example: analyzer/messages.yaml WarningCode.constantPatternNeverMatchesValueType
void f(int x) {
  if (x case true) {}
}
