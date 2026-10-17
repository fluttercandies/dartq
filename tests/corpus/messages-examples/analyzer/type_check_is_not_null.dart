// messages-example: analyzer/messages.yaml WarningCode.typeCheckIsNotNull
void f(String? s) {
  if (s is Null) {
    return;
  }
  print(s);
}
