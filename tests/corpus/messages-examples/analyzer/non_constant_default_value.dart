// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantDefaultValue
var defaultValue = 3;

void f([int value = defaultValue]) {}
