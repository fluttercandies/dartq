// messages-example: analyzer/messages.yaml WarningCode.patternNeverMatchesValueType
void f(String? s) {
  if (s case int _) {}
}
