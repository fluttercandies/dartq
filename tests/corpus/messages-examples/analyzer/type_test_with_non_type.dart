// messages-example: analyzer/messages.yaml CompileTimeErrorCode.typeTestWithNonType
typedef B = int Function(int);

void f(Object a, B b) {
  if (a is b) {
    return;
  }
}
