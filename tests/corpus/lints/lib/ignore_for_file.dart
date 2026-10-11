// ignore-face file-wide suppression: ignore_for_file applies from its
// line to the end of the file regardless of violation lines.
// Expected rows: 0 — expect-empty on both sides.
// ignore_for_file: await_only_futures

void f() async {
  await 42;
}
