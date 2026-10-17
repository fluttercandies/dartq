// Targets dead_code: statements after a terminating return are unreachable
// (dead_code_verifier's dead-interval model).
void f() {
  return;
  1;
}

// Live controls (appended for ticket #1380, return-only): a `return` only
// kills the flow of its own arm or scope — upstream stays silent where paths
// join or scopes differ. Upstream has no return-specific exemption; its only
// explicit silent shapes (dead_code_verifier.dart flowEnd, lines 239-251:
// assert message, constructor body) are anchor exemptions, not return shapes,
// so the control pins flow locality instead.
//
// Upstream anchors (dart-sdk reference sources):
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
//   handle_ifStatement_thenEnd (line 1203): flowEnd on the then-arm rejoins
//   the arms' flows, so a statement after the `if` is visited with the joined,
//   reachable flow.
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
//   visitReturnStatement (line 4412): handleReturn marks only the current
//   flow context unreachable; inside a function expression it feeds the
//   anonymous method's return model (flow_analysis.dart handleReturn,
//   line 7019-7028), never the enclosing function's flow.
// - pkg/analyzer/lib/src/dart/resolver/function_expression_resolver.dart
//   (line 82): the closure's dead interval, if any, closes at the closure's
//   own flowEnd and cannot leak outward.
// - pkg/analyzer/lib/src/error/dead_code_verifier.dart, _visitNode (line 524):
//   a node is recorded dead only when flow.isReachable is false at its own
//   visit — joined and outer flows stay reachable, so no dead_code.
//
// Deliberate silences, approximate (the serial N2 oracle run pins exact
// counts):
// - liveReturnControls: zero rows — `print(1)` after the if and `print(2)`
//   after the closure call are both live; proves the deadness in `f` above is
//   fed by the return's own sequence, not by the shape "return in a function".
void liveReturnControls(bool c) {
  if (c) {
    return;
  }
  print(1);
  runWith(() {
    return;
  });
  print(2);
}

void runWith(void Function() f) {}
