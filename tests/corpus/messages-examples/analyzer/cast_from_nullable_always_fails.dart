// messages-example: analyzer/messages.yaml WarningCode.castFromNullableAlwaysFails
void f() {
  num? x;
  x as int;
  print(x);
}
