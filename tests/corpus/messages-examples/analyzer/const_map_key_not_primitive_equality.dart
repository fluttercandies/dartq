// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constMapKeyNotPrimitiveEquality
class C {
  const C();

  @override
  bool operator ==(Object other) => true;
}

const map = {C(): 0};
