// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidSuperFormalParameterLocation (variant 2)
class A {
  A(int x);
}

class C extends A {
  factory C.c(super.x) => C._();
  C._() : super(0);
}
