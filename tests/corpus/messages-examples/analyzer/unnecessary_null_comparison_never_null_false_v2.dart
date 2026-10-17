// messages-example: analyzer/messages.yaml WarningCode.unnecessaryNullComparisonNeverNullFalse (variant 2)
void f(int x) {
  if (x == null) {
    throw ArgumentError("x can't be null");
  }
}
