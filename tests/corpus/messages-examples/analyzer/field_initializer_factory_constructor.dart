// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializerFactoryConstructor
class C {
  int? f;

  factory C(this.f) => throw 0;
}
