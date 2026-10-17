// messages-example: analyzer/messages.yaml CompileTimeErrorCode.redirectToNonConstConstructor
class C {
  const C.a() : this.b();
  C.b();
}
