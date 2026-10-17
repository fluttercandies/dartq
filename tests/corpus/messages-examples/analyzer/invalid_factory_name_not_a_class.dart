// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidFactoryNameNotAClass
// @dart = 3.12
class A {}

class C {
  factory A() => throw 0;
}
