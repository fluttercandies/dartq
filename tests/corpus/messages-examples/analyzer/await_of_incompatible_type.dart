// messages-example: analyzer/messages.yaml CompileTimeErrorCode.awaitOfIncompatibleType
extension type E(int i) {}

void f(E e) async {
  await e;
}
