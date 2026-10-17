// messages-example: analyzer/messages.yaml WarningCode.unusedCatchClause
void f() {
  try {
    int.parse(';');
  } on FormatException catch (e) {
    // ignored
  }
}
