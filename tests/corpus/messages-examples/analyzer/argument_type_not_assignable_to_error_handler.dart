// messages-example: analyzer/messages.yaml WarningCode.argumentTypeNotAssignableToErrorHandler
void f(Future<int> f) {
  f.catchError(() => 0);
}
