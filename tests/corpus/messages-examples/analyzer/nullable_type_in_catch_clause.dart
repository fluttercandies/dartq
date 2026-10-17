// messages-example: analyzer/messages.yaml WarningCode.nullableTypeInCatchClause
void f() {
  try {
    // ...
  } on FormatException? {
    // ...
  }
}
