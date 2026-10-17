// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionOverrideWithoutAccess
extension E on int {
  int get a => 0;
}

void f(int i) {
  print(E(i));
}
