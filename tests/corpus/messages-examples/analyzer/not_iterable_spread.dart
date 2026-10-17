// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notIterableSpread
var m = <String, int>{'a': 0, 'b': 1};
var s = <String>{...m};
