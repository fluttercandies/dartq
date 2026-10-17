// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalConcreteEnumMemberDeclaration (variant 2)
abstract class C implements Enum {
  @override
  int hashCode = 0;
}
