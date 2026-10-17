// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notMapSpread
var l = <String>['a', 'b'];

var m = <int, String>{...l};
