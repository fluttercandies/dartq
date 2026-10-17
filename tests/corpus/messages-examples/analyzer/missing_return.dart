// messages-example: analyzer/messages.yaml WarningCode.missingReturn
int f(int x) {
  if (x < 0) {
    return 0;
  }
}
