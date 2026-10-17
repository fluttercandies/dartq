// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinInheritsFromNotObject
//@dart=2.19
class A {}

class B extends A {}

class C with B {}
