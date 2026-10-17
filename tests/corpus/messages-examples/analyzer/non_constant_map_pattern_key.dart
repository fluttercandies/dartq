// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantMapPatternKey
void f(Object x) {
  if (x case {A(): 0}) {}
}

class A {
  const A();
}
