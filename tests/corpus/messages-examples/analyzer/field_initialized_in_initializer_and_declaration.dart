// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializedInInitializerAndDeclaration
class C {
  final int f = 0;
  C() : f = 1;
}
