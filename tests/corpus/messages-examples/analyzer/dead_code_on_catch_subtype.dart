// messages-example: analyzer/messages.yaml WarningCode.deadCodeOnCatchSubtype
void f() {
  try {
    // ...
  } on num {
    // ...
  } on int {
    // ...
  }
}
