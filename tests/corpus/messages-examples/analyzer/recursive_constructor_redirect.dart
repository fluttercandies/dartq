// messages-example: analyzer/messages.yaml CompileTimeErrorCode.recursiveConstructorRedirect
class C {
  C.a() : this.b();
  C.b() : this.a();
}
