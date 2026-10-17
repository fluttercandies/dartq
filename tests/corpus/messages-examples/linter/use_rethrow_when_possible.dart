// messages-example: linter/messages.yaml LinterLintCode.useRethrowWhenPossible
void f() {
  try {
    // ...
  } catch (e) {
    throw e;
  }
}
