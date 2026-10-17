// messages-example: linter/messages.yaml LinterLintCode.recursiveGetters
class C {
  int _count = 0;

  int get count => count;
}
