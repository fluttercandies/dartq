// messages-example: analyzer/messages.yaml WarningCode.sdkVersionEqEqOperatorInConstContext
class C {}
const C a = null;
const C b = null;
const bool same = a == b;
