// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedEnumConstructorNamed
enum E {
  c();

  const E.x();
}
