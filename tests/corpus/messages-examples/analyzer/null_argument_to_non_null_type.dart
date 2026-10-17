// messages-example: analyzer/messages.yaml WarningCode.nullArgumentToNonNullType
Future<String> f() {
  return Future.value(null);
}
