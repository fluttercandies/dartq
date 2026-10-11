// ignore-face CONTROL — the same violation the suppression files carry,
// with no suppression comment. Proves the rule fires on both sides so
// the expect-empty suppression files are meaningful (paired design:
// the (code, file) grain cannot see suppression without this control).
// Expected rows: 1 — AWAIT_ONLY_FUTURES keyed ignore_control.dart.
void f() async {
  await 42;
}
