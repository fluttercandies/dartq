// messages-example: linter/messages.yaml LinterLintCode.literalOnlyBooleanExpressions (variant 2)
void g(int i) {
  if (1 == 0 || 3 > 4) {
    print('false');
  }
}
