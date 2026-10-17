// messages-example: analyzer/messages.yaml CompileTimeErrorCode.caseExpressionTypeImplementsEquals
class C {
  final int value;

  const C(this.value);

  @override
  bool operator ==(Object other) {
    return false;
  }
}

void f(C c) {
  switch (c) {
    case C(0):
      break;
  }
}
