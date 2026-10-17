// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constConstructorWithNonConstSuper
class A {
  const A();
  A.nonConst();
}

class B extends A {
  const B() : super.nonConst();
}
