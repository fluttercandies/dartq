// parameter_assignments — official rule faces
// (pkg/linter parameter_assignments.dart:94-112, test/rules/
// parameter_assignments_test.dart:140,156,164,180-231,319-328): any write
// to a function's own parameter inside its body reports at the write,
// with one order-sensitive exemption: on an optional parameter without a
// default clause the FIRST `??=` is the null-replacement initializer
// idiom and stays silent — every later write on that parameter,
// including a second `??=`, reports.
//
// EXPECTED-AFTER-#1135-FIX POSTURE: adjudication #1135 (option A1) aligns
// the second-`??=` face. The expectations below are written to the
// post-fix target behavior: until the fix lands, dartq under-reports
// exactly the marked A1 row (the canonical double-`??=` face) while the
// oracle reports it, so a first gate run over the judged universe is
// expected red on that row and turns green exactly when the fix lands.
// Every other row is parity today.
//
// Expected rows: 6 — PARAMETER_ASSIGNMENTS keyed parameter_assignments.dart,
// message "Invalid assignment to the parameter '{0}'.":
//   the `p = 4` write (required parameter)
//   the `p++` write (named parameter with a default)
//   the second `optional ??= 16` write (null-default face, A1 target)
//   the `p ??= 8` write (non-null defaulted `[int p = 42]`)
//   the `optional += 16` write after a first `??=`
//   the `optional = 16` write after a first `??=`
void f(int p) {
  p = 4;
}

void g({int p = 5}) {
  print(p++);
}

// The canonical double-`??=` face (upstream test
// test_function_positional_optional_assignedTwice): the first write is
// the initializer idiom, the second reports. The oracle additionally
// emits two flow-analysis warnings on the second write
// (dead_null_aware_expression and deadCode) — the semantic engine lane's
// faces, not lint expectations; the lint gate's severity/type filter
// never judges them here.
void h([int? optional]) {
  optional ??= 8;
  optional ??= 16;
}

// A defaulted parameter is in the non-null-default regime, so even the
// first `??=` reports (upstream test
// test_nullAwareAssignment_nonNullableParameter). The oracle also emits
// the two flow-analysis warnings here; same filter story as above.
void i([int p = 42]) {
  p ??= 8;
}

// A non-`??=` write after a first `??=` reports immediately (upstream
// tests test_function_positional_optional_re_incremented and
// ..._reassigned).
void j([int? optional]) {
  optional ??= 8;
  optional += 16;
}

void k([int? optional]) {
  optional ??= 8;
  optional = 16;
}

// GOOD: a single `??=` on a default-less optional parameter is the
// initializer idiom, positional and named alike (upstream tests
// test_function_positional_optional_ok and
// test_function_named_optional_ok).
void l([int? optional]) {
  optional ??= 8;
}

void m({int? optional}) {
  optional ??= 8;
}

// GOOD: a shadowing local keeps its own writes inert (upstream test
// test_function_ok_shadow).
void n(String? p) {
  if (p == null) {
    int p = 2;
    p = 3;
  }
}
