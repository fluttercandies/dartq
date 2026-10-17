// messages-example: analyzer/messages.yaml CompileTimeErrorCode.relationalPatternOperatorReturnTypeNotAssignableToBool
class C {
  const C();

  int operator >(C c) => 3;

  bool operator <(C c) => false;
}

const C c2 = C();

void f(C c1) {
  if (c1 case > c2) {}
}
