// messages-example: analyzer/messages.yaml CompileTimeErrorCode.referencedBeforeDeclaration
void f() {
  print(i);
  int i = 5;
}
