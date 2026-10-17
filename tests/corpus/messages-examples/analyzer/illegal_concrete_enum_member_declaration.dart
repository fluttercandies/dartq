// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalConcreteEnumMemberDeclaration
enum E {
  v;

  @override
  int get index => 0;
}
