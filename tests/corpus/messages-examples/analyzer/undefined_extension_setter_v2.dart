// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedExtensionSetter (variant 2)
extension E on String {}

void f() {
  E.a = 3;
}
