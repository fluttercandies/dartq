// curly_braces_in_flow_control_structures — brace-free flow-control
// bodies. The single-line no-else `if` exemption (official :62-69, dartq
// check_if line comparison) is deliberately NOT exercised as a violation
// row: both sides exempt it, so a plain single-line `if` would be silent.
// The single-line if/else IS a violation row — the else-arm disables the
// line exemption on both sides.
// Expected rows: 6 — CURLY_BRACES_IN_FLOW_CONTROL_STRUCTURES keyed
// curly_braces.dart: multi-line if (1), single-line if/else then-arm (1)
// and else-arm (1), while (1), for (1), do (1).
void f(bool c) {
  if (c)
    print(0);
  if (c) print(1); else print(2);
  while (c) print(3);
  for (var i = 0; i < 1; i++) print(i);
  do print(4); while (c);
}
