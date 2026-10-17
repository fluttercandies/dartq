// messages-example: analyzer/messages.yaml CompileTimeErrorCode.disallowedTypeInstantiationExpression
int i = 1;

void f() {
  print(i<int>);
}
