// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constSetElementNotPrimitiveEquality (variant 2)
class C {
  const C();

  @override
  int get hashCode => 3;
}

const map = {C()};
