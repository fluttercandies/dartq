// messages-example: analyzer/messages.yaml CompileTimeErrorCode.conflictingConstructorAndStaticField (variant 2)
class C {
  C.foo();
  static void foo() {}
}
