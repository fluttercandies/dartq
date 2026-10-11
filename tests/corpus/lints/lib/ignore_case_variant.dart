// ignore-face case-variant rule spelling: both sides match ignore
// targets case-insensitively (analyzer ignore_info.dart lowercases the
// name; dartq ignore.rs eq_ignore_ascii_case), so the uppercase spelling
// still suppresses.
// Expected rows: 0 — expect-empty on both sides.
void f() async {
  // ignore: AWAIT_ONLY_FUTURES
  await 42;
}
