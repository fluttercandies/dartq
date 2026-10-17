// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializerRedirectingConstructor
class C {
  int f;

  C(this.f);

  C.zero(this.f) : this(f);
}
