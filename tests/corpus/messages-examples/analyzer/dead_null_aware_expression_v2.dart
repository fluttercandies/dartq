// messages-example: analyzer/messages.yaml StaticWarningCode.deadNullAwareExpression (variant 2)
class C {
  int f = -1;

  void m(int x) {
    f ??= x;
  }
}
