// messages-example: linter/messages.yaml LinterLintCode.controlFlowInFinally
void f(void Function() g) {
  try {
    g();
  } catch (e) {
    print(e);
  } finally {
    return;
  }
}
