// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfTypeArgumentsElement
class C {
  int m<A>(A a) => 0;
}

int f(C c) => c.m<int, int>(2);
