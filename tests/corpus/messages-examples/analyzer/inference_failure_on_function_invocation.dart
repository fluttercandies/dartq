// messages-example: analyzer/messages.yaml WarningCode.inferenceFailureOnFunctionInvocation
abstract class C {
  void m<T>();
}

void f(C c) {
  c.m();
}
