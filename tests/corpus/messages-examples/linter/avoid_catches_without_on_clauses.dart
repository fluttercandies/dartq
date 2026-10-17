// messages-example: linter/messages.yaml LinterLintCode.avoidCatchesWithoutOnClauses
void f() {
  try {
    int.parse('?');
  } catch (e) {
    print(e);
  }
}
