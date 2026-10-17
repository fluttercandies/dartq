// messages-example: analyzer/messages.yaml CompileTimeErrorCode.positionalFieldInObjectPattern
void f(Object o) {
  if (o case String(1)) {}
}
