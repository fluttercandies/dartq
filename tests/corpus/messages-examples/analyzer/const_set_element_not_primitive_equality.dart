// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constSetElementNotPrimitiveEquality
class C {
  const C();

  @override
  bool operator ==(Object other) => true;
}

const set = {C()};
