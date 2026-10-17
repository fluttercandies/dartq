// messages-example: analyzer/messages.yaml WarningCode.unnecessaryFinal (variant 2)
// @dart = 3.11
class A {
  A(int value);
}

class B extends A {
  B(final super.value);
}
