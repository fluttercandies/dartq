// messages-example: analyzer/messages.yaml WarningCode.unusedLabel
void f(int limit) {
  loop:
  for (int i = 0; i < limit; i++) {
    print(i);
  }
}
