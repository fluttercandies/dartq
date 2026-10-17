// messages-example: analyzer/messages.yaml WarningCode.unreachableSwitchDefault
enum E { e1, e2 }

void f(E x) {
  switch (x) {
    case E.e1:
      print('one');
    case E.e2:
      print('two');
    default:
      print('other');
  }
}
