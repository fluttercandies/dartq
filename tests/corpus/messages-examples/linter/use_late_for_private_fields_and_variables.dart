// messages-example: linter/messages.yaml LinterLintCode.useLateForPrivateFieldsAndVariables
void f() {
  _i!.abs();
}

int? _i;
