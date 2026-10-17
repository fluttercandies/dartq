// messages-example: analyzer/messages.yaml CompileTimeErrorCode.assertInRedirectingConstructor
class C {
  C(int x) : assert(x > 0), this.name();
  C.name();
}
