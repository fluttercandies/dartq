// throw_in_finally — official rule faces
// (pkg/linter throw_in_finally.dart with the shared
// ControlFlowInFinallyBlockReporter): a throw whose nearest enclosing try
// statement has a finally block and whose chain to it stays inside that
// block reports, unless the throw sits inside a nested function body
// within the block (then it is that closure's own control flow).
// Expected rows: 1 — THROW_IN_FINALLY keyed throw_in_finally.dart, message
// "Use of 'throw' in a 'finally' block.":
//   the `throw 1` inside the finally block
void f() {
  try {} finally {
    throw 1;
  }
}

// GOOD: a throw in the try block, and one inside a nested closure of the
// finally block, pass.
void g() {
  try {
    throw 1;
  } finally {}
}

void h() {
  try {} finally {
    p(() {
      throw 1;
    });
  }
}

void p(void Function() callback) {}
