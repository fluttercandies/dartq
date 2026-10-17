// messages-example: analyzer/messages.yaml CompileTimeErrorCode.awaitInLateLocalVariableInitializer
Future<int> f(Future<int> future) async {
  late var v = await future;
  return v;
}
