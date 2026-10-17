// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinClassDeclaresConstructor
//@dart=2.19
class A {
  A();
}

class B with A {}
