// messages-example: analyzer/messages.yaml WarningCode.unnecessaryNullComparisonNeverNullFalse
void f(int x) {
  if (x != null) {
    print(x);
  }
}
