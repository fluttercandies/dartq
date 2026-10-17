// messages-example: analyzer/messages.yaml StaticWarningCode.invalidNullAwareOperator
int? getLength(String s) {
  return s?.length;
}
