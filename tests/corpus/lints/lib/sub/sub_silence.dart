// Subtree silence face (paired with the root control
// lib/await_only_futures.dart): await_only_futures is disabled ONLY via
// lib/sub/analysis_options.yaml, so the same violation is silent inside
// the subtree — on both sides. A dartq regression in per-subtree
// enable-set resolution surfaces this file as a dartq-only gate row.
// Expected rows: 0 — expect-empty on both sides.
void subSilence() async {
  await 42;
}
