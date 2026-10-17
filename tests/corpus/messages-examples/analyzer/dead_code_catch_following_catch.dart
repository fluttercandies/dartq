// messages-example: analyzer/messages.yaml WarningCode.deadCodeCatchFollowingCatch
void f() {
  try {
    // ...
  } catch (e) {
    // Not reached.
  } on String {
    // ...
  }
}
