// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonFinalFieldInEnum
enum E {
  c;

  int f = 0;
}
