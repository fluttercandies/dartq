// messages-example: analyzer/messages.yaml WarningCode.argumentTypeNotAssignableToErrorHandler (variant 3)
void f(Future<int> f) {
  f.catchError((String error) => 0);
}
