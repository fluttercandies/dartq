// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonTypeInCatchClause
void f() {
  try {
    // ...
  } on f {
    // ...
  }
}
