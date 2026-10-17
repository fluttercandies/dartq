// messages-example: analyzer/messages.yaml WarningCode.sdkVersionIsExpressionInConstContext
const Object x = 4;
const y = x is int ? 0 : 1;
