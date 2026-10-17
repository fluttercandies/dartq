// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invocationOfNonFunction
typedef Binary = int Function(int, int);

int f() {
  return Binary(1, 2);
}
