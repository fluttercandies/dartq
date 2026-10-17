// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternTypeMismatchInIrrefutableContext
void f(Object x) {
  var String(length: a) = x;
  print(a);
}
