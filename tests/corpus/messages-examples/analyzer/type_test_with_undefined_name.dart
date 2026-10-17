// messages-example: analyzer/messages.yaml CompileTimeErrorCode.typeTestWithUndefinedName
void f(Object o) {
  if (o is Srting) {
    // ...
  }
}
