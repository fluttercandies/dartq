// messages-example: analyzer/messages.yaml WarningCode.returnTypeInvalidForThen
void f(Future<int> future) {
  future.then((_) => 0, onError: (e, st) => 'c');
}
