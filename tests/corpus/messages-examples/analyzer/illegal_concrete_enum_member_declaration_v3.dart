// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalConcreteEnumMemberDeclaration (variant 3)
abstract class A implements Enum {}

abstract class C implements A {
  @override
  int get hashCode => 0;
}
