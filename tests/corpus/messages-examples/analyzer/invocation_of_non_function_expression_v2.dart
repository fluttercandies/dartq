// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invocationOfNonFunctionExpression (variant 2)
int x = 0;

int f() => x;

var y = f()();
