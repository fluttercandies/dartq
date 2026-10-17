// messages-example: analyzer/messages.yaml StaticWarningCode.invalidNullAwareOperator (variant 3)
void f(String? s) {
  s?.length?.isEven;
}
