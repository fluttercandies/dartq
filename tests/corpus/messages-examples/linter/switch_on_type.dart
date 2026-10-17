// messages-example: linter/messages.yaml LinterLintCode.switchOnType
void f(Object o) {
  switch (o.runtimeType) {
    case const (int):
      print('int');
    case const (String):
      print('String');
  }
}
