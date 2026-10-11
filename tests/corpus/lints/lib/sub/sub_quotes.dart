// Subtree enablement face: prefer_single_quotes is enabled ONLY via
// lib/sub/analysis_options.yaml (the oracle runs the real SDK rule on
// this file; dartq has no implementation — every row sits outside the
// frozen universe codes file, gate-invisible by design).
// Expected rows: oracle 1 PREFER_SINGLE_QUOTES; dartq none. Filtered by
// the gate's codes file either way.
void subQuotes() {
  final quoted = "double";
  print(quoted);
}
