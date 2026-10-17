// messages-example: analyzer/messages.yaml CompileTimeErrorCode.multipleSuperInitializers
class A {
  int? x;
  String? s;
  A.one(this.x);
  A.two(this.s);
}

class B extends A {
  B() : super.one(0), super.two('');
}
