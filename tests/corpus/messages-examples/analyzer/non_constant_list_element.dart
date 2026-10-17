// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantListElement
var x = 2;
var y = const <int>[0, 1, x];
