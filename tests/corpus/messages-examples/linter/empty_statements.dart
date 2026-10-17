// messages-example: linter/messages.yaml LinterLintCode.emptyStatements
void f(bool condition) {
  while (condition) {
    ;
  }
  g();
}

void g() {}
