// messages-example: analyzer/messages.yaml CompileTimeErrorCode.relationalPatternOperandTypeNotAssignable
class C {
  const C();

  bool operator >(C other) => true;
}

void f(C c) {
  switch (c) {
    case > 0:
      print('positive');
  }
}
