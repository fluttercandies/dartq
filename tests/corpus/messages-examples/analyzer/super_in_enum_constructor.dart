// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superInEnumConstructor
enum E {
  e;

  const E() : super();
}
