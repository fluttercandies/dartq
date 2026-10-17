// messages-example: analyzer/messages.yaml CompileTimeErrorCode.implementsSuperClass (variant 2)
mixin class A {}

class B extends A with A {}
