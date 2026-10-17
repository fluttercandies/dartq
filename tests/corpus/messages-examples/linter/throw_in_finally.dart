// messages-example: linter/messages.yaml LinterLintCode.throwInFinally
void f() {
  try {
    // ...
  } catch (e) {
    // ...
  } finally {
    throw 'error';
  }
}
