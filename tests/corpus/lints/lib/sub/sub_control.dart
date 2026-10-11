// Subtree include-chain control: proves the subtree options resolved
// their include (this rule fires from the parent's chain, inside the
// subtree). Brace-less while body — the curly_braces row (the
// single-line no-else `if` exemption would be silent on both sides).
// Expected rows: 1 — CURLY_BRACES_IN_FLOW_CONTROL_STRUCTURES keyed
// lib/sub/sub_control.dart.
void subControl(bool flag) {
  while (flag) print(0);
}
