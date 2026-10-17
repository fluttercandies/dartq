// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constConstructorWithNonFinalField
class C {
  int x;

  const C(this.x);
}
