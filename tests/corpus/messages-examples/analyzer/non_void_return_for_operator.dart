// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonVoidReturnForOperator
class C {
  int operator []=(int index, int value) => 0;
}
