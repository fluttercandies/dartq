// messages-example: analyzer/messages.yaml CompileTimeErrorCode.enumConstantWithNonConstConstructor
enum E {
  e();

  factory E() => e;
}
