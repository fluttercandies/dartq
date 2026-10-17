// messages-example: analyzer/messages.yaml CompileTimeErrorCode.finalInitializedInDeclarationAndConstructor
class C {
  final int f = 0;

  C(this.f);
}
