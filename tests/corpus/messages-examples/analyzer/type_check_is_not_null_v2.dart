// messages-example: analyzer/messages.yaml WarningCode.typeCheckIsNotNull (variant 2)
void f(String? s) {
  if (s is! Null) {
    print(s);
  }
}
