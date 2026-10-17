// messages-example: linter/messages.yaml LinterLintCode.avoidEmptyElse
void f(int x, int y) {
  if (x > y) {
    print('1');
  } else
    ;
  print('2');
}
