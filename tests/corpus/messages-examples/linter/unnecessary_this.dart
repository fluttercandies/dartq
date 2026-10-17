// messages-example: linter/messages.yaml LinterLintCode.unnecessaryThis
class C {
  final int _f = 2;

  int get f => this._f;
}
