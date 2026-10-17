// messages-example: analyzer/messages.yaml WarningCode.unnecessaryCast
void f(num n) {
  if (n is int) {
    (n as int).isEven;
  }
}
