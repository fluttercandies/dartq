// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalEnumValuesDeclaration
abstract class C implements Enum {
  int get values => 0;
}
