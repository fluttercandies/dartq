// messages-example: analyzer/messages.yaml CompileTimeErrorCode.fieldInitializerRedirectingConstructor (variant 2)
class C {
  int f;

  C(this.f);

  C.zero() : f = 0, this(1);
}
