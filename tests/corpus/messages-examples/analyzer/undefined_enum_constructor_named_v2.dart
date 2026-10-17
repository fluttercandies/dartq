// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedEnumConstructorNamed (variant 2)
enum E {
  c.x();

  const E.y();
}
