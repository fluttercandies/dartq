// messages-example: analyzer/messages.yaml CompileTimeErrorCode.instanceMemberAccessFromFactory
class C {
  int x;
  factory C() {
    return C._(x);
  }
  C._(this.x);
}
