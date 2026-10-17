// messages-example: linter/messages.yaml LinterLintCode.unnecessaryConstInEnumConstructor (variant 2)
enum E {
  a(1),
  b(2);

  const E(this.i);

  final int i;
}
