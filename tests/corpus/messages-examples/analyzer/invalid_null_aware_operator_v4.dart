// messages-example: analyzer/messages.yaml StaticWarningCode.invalidNullAwareOperator (variant 4)
void f(Object? o) {
  var s = o as String;
  s?.length;
}
