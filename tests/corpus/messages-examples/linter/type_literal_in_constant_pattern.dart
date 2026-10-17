// messages-example: linter/messages.yaml LinterLintCode.typeLiteralInConstantPattern
void f(Object? x) {
  if (x case num) {
    // ...
  }
}
