// messages-example: analyzer/messages.yaml WarningCode.deprecatedFactoryMethod
// @dart=3.10
class C {
  factory() => C();
}
