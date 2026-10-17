// messages-example: linter/messages.yaml LinterLintCode.avoidUnusedConstructorParameters (variant 2)
class C {
  int x;

  C(int a, int b, int c) : x = a + b {
    print(x);
  }
}
