// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedExtensionOperator
var x = E('') * 4;

extension E on String {}
