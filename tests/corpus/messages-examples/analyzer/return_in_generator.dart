// messages-example: analyzer/messages.yaml CompileTimeErrorCode.returnInGenerator
Iterable<int> f() sync* {
  return 3;
}
