// messages-example: analyzer/messages.yaml WarningCode.unnecessaryWildcardPattern
void f(Object? x) {
  if (x case _ && 0) {}
}
