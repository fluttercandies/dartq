// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalAsyncReturnType
int f() async {
  return 0;
}
