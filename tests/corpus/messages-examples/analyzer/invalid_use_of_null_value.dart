// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidUseOfNullValue
int f(Null x) {
  return x.length;
}
