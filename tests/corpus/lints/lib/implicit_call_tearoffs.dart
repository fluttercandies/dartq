// implicit_call_tearoffs — audit r1 likely-blind row, kept to the dartq
// documented ported face: assignment/initializer site, written
// FunctionType target, right side a bare or `this.` field access whose
// type names a same-file class with an instance `call` method. Argument,
// return and constructor-invocation sites are the dominant dartq
// under-report face and stay out of shape.
// Expected rows: 2 — IMPLICIT_CALL_TEAROFFS keyed
// implicit_call_tearoffs.dart (`= c` bare field; `= this.c` prefixed
// field). Face re-validated against the oracle at the r0 run. Co-fire:
// UNNECESSARY_THIS on the `this.c` receiver (line 19; same no-import
// face); the prefixed field stays — it carries a dartq pin value.
class Callable {
  void call() {}
}

class TearoffSites {
  Callable c = Callable();

  void run() {
    void Function() p = c;
    void Function() q = this.c;
    print(p);
    print(q);
  }
}
