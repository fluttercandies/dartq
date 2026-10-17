// messages-example: analyzer/messages.yaml CompileTimeErrorCode.primaryConstructorCannotRedirect
class C(int x) {
  C.named() : this(0);
  this : assert(x > 0), this.named();
}
