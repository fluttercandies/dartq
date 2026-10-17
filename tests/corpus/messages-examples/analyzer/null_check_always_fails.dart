// messages-example: analyzer/messages.yaml WarningCode.nullCheckAlwaysFails
void f() {
  g()!;
}

Null g() => null;
