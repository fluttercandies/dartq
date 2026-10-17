// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedIdentifierAwait
void f(Future<void> p) {
  await p;
}
