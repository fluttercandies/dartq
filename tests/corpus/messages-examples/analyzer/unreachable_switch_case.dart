// messages-example: analyzer/messages.yaml WarningCode.unreachableSwitchCase
void f(int x) {
  switch (x) {
    case 1:
      print('one');
    case 1:
      print('two');
  }
}
