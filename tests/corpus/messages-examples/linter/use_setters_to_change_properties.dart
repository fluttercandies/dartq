// messages-example: linter/messages.yaml LinterLintCode.useSettersToChangeProperties
class C {
  int _f = 0;

  void setF(int value) => _f = value;
}
