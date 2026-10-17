// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalConcreteEnumMemberDeclaration (variant 4)
mixin M on Enum {
  @override
  bool operator ==(Object other) => false;
}
