// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidSuperFormalParameterLocation
class A {
  A(int x);
}

class B extends A {
  B.b(super.x) : this._();
  B._() : super(0);
}
