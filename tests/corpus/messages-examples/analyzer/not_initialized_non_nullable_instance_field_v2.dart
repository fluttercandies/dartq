// messages-example: analyzer/messages.yaml CompileTimeErrorCode.notInitializedNonNullableInstanceField (variant 2)
class C {
  int x;

  C(this.x);

  C.n();
}
