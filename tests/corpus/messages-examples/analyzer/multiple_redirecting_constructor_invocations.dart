// messages-example: analyzer/messages.yaml CompileTimeErrorCode.multipleRedirectingConstructorInvocations
class C {
  C() : this.a(), this.b();
  C.a();
  C.b();
}
