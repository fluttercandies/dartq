// messages-example: linter/messages.yaml LinterLintCode.useIfNullToConvertNullsToBools
void f(bool? b) {
  if (b == true) {
    // Treats `null` as `false`.
  }
}
