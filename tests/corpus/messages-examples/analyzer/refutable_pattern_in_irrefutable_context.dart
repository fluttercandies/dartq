// messages-example: analyzer/messages.yaml CompileTimeErrorCode.refutablePatternInIrrefutableContext
void f(int? x) {
  var (_?) = x;
}
