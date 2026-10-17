// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invocationOfExtensionWithoutCall
extension E on String {}

void f() {
  E('')();
}
