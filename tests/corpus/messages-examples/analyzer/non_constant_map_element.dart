// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantMapElement
var notConst = <int, int>{};
var map = const <int, int>{...notConst};
