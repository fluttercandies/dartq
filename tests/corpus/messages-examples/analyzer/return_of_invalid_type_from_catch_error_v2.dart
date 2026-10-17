// messages-example: analyzer/messages.yaml WarningCode.returnOfInvalidTypeFromCatchError (variant 2)
void f(Future<String> future) {
  future.catchError((error, stackTrace) => 3);
}
