// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantRelationalPatternExpression
final a = 0;

void f(int x) {
  if (x case > a) {}
}
