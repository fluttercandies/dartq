// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superInvocationNotLast
class A {
  A(int x);
}

class B extends A {
  B(int x) : super(x + 1), assert(x >= 0);
}
