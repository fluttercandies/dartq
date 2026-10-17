// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constConstructorParamTypeMismatch
class C {
  final String s;

  const C(this.s);
}

const dynamic i = 0;

void f() {
  const C(i);
}
