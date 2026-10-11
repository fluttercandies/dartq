// ignore-face own-line suppression. The ignore comment sits on the line
// immediately preceding the violation line (the analyzer suppression
// window: own line, preceding line, or the violation's line).
// Expected rows: 0 — expect-empty on both sides.
void f() async {
  // ignore: await_only_futures
  await 42;
}
