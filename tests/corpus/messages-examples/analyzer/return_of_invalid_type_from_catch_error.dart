// messages-example: analyzer/messages.yaml WarningCode.returnOfInvalidTypeFromCatchError
void f(Future<int> future, String Function(dynamic, StackTrace) callback) {
  future.catchError(callback);
}
