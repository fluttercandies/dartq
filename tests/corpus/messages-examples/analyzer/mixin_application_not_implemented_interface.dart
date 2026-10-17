// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinApplicationNotImplementedInterface
class A {}

mixin M on A {}

class X = Object with M;
