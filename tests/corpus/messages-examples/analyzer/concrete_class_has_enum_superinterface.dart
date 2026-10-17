// messages-example: analyzer/messages.yaml CompileTimeErrorCode.concreteClassHasEnumSuperinterface
abstract class A implements Enum {}

class B implements A {}
