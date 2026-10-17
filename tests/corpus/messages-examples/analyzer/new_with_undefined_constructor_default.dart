// messages-example: analyzer/messages.yaml CompileTimeErrorCode.newWithUndefinedConstructorDefault
class A {
  A.a();
}

A f() => A();
