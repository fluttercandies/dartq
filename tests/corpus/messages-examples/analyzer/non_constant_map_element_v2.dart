// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantMapElement (variant 2)
bool notConst = true;
var map = const <int, int>{if (notConst) 1: 2};
