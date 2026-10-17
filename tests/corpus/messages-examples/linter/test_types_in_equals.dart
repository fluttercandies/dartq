// messages-example: linter/messages.yaml LinterLintCode.testTypesInEquals
class C {
  final int f;

  C(this.f);

  @override
  bool operator ==(Object other) {
    return (other as C).f == f;
  }
}
