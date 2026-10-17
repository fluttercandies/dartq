// messages-example: analyzer/messages.yaml WarningCode.bodyMightCompleteNormallyCatchError
void g(Future<int> f) {
  f.catchError((e, st) {});
}
