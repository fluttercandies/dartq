// ignore-face trailing suppression: the ignore comment shares the
// violation's line.
// Expected rows: 0 — expect-empty on both sides.
void f() async {
  await 42; // ignore: await_only_futures
}
