// control_flow_in_finally — audit r1 thin row. The ledgered forward
// `continue L` face (unresolved member-label target) stays out; only the
// plain break/return faces, whose jump targets sit outside the try.
// Expected rows: 2 — CONTROL_FLOW_IN_FINALLY keyed
// control_flow_in_finally.dart (break in finally; return in finally).
void f(bool c) {
  while (c) {
    try {
      print(0);
    } finally {
      break;
    }
  }
  try {
    print(1);
  } finally {
    return;
  }
}
