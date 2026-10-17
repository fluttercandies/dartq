// messages-example: linter/messages.yaml LinterLintCode.unnecessaryGettersSetters
class C {
  int? _c;

  int? get c => _c;

  set c(int? v) => _c = v;
}
