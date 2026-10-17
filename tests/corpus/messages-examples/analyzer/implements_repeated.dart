// messages-example: analyzer/messages.yaml CompileTimeErrorCode.implementsRepeated
class A {}

class B implements A, A {}
