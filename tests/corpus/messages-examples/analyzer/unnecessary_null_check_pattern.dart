// messages-example: analyzer/messages.yaml StaticWarningCode.unnecessaryNullCheckPattern
void f(int x) {
  if (x case var a? when a > 0) {}
}
