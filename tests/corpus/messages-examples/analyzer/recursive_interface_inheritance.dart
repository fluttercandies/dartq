// messages-example: analyzer/messages.yaml CompileTimeErrorCode.recursiveInterfaceInheritance
class A extends B {}

class B implements A {}
