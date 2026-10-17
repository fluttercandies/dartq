// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionAsExpression
extension E on int {
  static String m() => '';
}

var x = E;
