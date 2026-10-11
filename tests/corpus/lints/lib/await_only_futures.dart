// await_only_futures — audit r1 blind-spot row (0 dedicated fixtures).
// Expected rows: 1 — AWAIT_ONLY_FUTURES keyed await_only_futures.dart
// (await of an `int` scalar; both sides report, dartq pin
// await_only_futures_scalar_literals_report uses the same shape).
void f() async {
  await 42;
}
