// messages-example: analyzer/messages.yaml StaticWarningCode.invalidNullAwareOperator (variant 5)
List<String> makeSingletonList(String s) {
  return <String>[?s];
}
