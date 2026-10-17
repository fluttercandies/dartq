// messages-example: analyzer/messages.yaml CompileTimeErrorCode.wrongNumberOfParametersForOperator
class C {
  int operator +(int a, int b) => 0;
}
