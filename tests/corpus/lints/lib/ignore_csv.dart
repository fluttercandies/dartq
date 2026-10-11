// ignore-face comma-separated list: one comment suppressing two rules.
// Both suppressed violations live on the covered line so both CSV
// entries are used (an unused entry would be its own oracle-side hint —
// HINT type, outside the gate's LINT filter, but avoid the noise).
// Expected rows: 0 — expect-empty on both sides.
void f() async {
  await 42; final bad = RegExp(r'(['); // ignore: await_only_futures, valid_regexps
}
