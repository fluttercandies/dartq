// messages-example: analyzer/messages.yaml CompileTimeErrorCode.illegalEnumValuesDeclaration (variant 2)
abstract class A {
  int values() => 0;
}

abstract class B extends A implements Enum {}
