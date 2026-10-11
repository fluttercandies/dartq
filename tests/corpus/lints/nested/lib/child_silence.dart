// Child-set silence face (paired with the root control
// lib/await_only_futures.dart): the child options disable
// await_only_futures relative to the parent's include chain, so the same
// violation is silent inside the nested package — on both sides. A dartq
// regression that ignores the child disable surfaces this file as a
// dartq-only gate row.
// Expected rows: 0 — expect-empty on both sides.
void childSilence() async {
  await 42;
}
