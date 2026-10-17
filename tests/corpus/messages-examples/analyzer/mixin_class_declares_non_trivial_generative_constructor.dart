// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinClassDeclaresNonTrivialGenerativeConstructor
mixin class A {
  A() : assert(true);
}
