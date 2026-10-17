// messages-example: analyzer/messages.yaml CompileTimeErrorCode.noCombinedSuperSignature
abstract class A {
  A m(String a);
}

abstract class B {
  B m(int a);
}

abstract class C implements A, B {
  m(a);
}
