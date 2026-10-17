// messages-example: linter/messages.yaml LinterLintCode.hashAndEquals
class C {
  final int value;

  C(this.value);

  @override
  bool operator ==(Object other) =>
      other is C && other.runtimeType == runtimeType && other.value == value;
}
