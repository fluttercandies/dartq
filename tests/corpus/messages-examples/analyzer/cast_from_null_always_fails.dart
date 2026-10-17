// messages-example: analyzer/messages.yaml WarningCode.castFromNullAlwaysFails
void f(Null n) {
  n as int;
}
