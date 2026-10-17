// messages-example: analyzer/messages.yaml CompileTimeErrorCode.abstractSuperMemberReference
abstract class A {
  int get a;
}

class B extends A {
  @override
  int get a => super.a;
}
