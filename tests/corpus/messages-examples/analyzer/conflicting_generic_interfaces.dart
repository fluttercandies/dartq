// messages-example: analyzer/messages.yaml CompileTimeErrorCode.conflictingGenericInterfaces
class I<T> {}

class A implements I<int> {}

class B implements I<String> {}

class C extends A implements B {}
