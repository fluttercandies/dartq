// messages-example: linter/messages.yaml LinterLintCode.unnecessaryTypeNameInConstructor (variant 2)
class C {
  factory C() => C._();

  new _();
}
