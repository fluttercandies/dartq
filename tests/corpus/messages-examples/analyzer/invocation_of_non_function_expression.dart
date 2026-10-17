// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invocationOfNonFunctionExpression
int x = 0;

int f() => x;

var y = x();
