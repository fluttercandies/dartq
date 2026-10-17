// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superInRedirectingConstructor
class C {
  C.a() : this.b(), super();
  C.b();
}
