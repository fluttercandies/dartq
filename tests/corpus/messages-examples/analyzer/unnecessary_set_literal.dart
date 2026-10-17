// messages-example: analyzer/messages.yaml WarningCode.unnecessarySetLiteral
void f() {
  g(() => {1});
}

void g(void Function() p) {}
