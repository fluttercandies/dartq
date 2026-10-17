// messages-example: linter/messages.yaml LinterLintCode.tightenTypeOfInitializingFormals
class C {
  final String? s;

  C(this.s) : assert(s != null);
}
