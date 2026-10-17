// messages-example: analyzer/messages.yaml WarningCode.argumentTypeNotAssignableToErrorHandler (variant 2)
void f(Future<int> f) {
  f.catchError((one, two, three) => 0);
}
