// Targets missing_enum_constant_in_switch: a switch statement over an enum
// that omits one constant and carries no default clause (analyzer
// error_verifier's enum-switch completeness pass). Version note: the legacy
// check is skipped when the patterns feature is enabled, and at this
// corpus's language 3.13 pin the oracle routes the shape through
// exhaustiveness checking and reports non_exhaustive_switch_statement (an
// out-of-universe code) instead — the frozen WARNING-tier code itself fires
// only pre-patterns (language <= 2.19, upstream test's BeforePatterns
// suite). The face stays in the corpus so the shape is exercised: any
// dartq emission of the frozen code here becomes a visible gate diff
// rather than a blind spot, and neither side may emit in-universe rows.
enum Status { idle, running, done }

void f(Status s) {
  switch (s) {
    case Status.idle:
      break;
    case Status.running:
      break;
  }
}
