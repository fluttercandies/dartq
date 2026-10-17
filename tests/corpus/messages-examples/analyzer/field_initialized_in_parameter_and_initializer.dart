// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializedInParameterAndInitializer
class C {
  int f;

  C(this.f) : f = 0;
}
