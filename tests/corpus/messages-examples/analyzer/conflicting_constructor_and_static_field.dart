// messages-example: analyzer/messages.yaml CompileTimeErrorCode.conflictingConstructorAndStaticField
class C {
  C.foo();
  static int foo = 0;
}
