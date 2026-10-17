// messages-example: analyzer/messages.yaml WarningCode.returnTypeInvalidForThen (variant 2)
void f(Future<int> future, String Function(dynamic, StackTrace) cb) {
  future.then<int>((_) => 1, onError: cb);
}
