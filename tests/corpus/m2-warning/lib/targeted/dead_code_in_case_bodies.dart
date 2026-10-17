// Targets dead_code inside switch CASE bodies (ticket #1380, corpus gap audit
// face F3): each of the four jump forms followed by a further statement in the
// same case body makes that statement dead. Complements dead_code.dart, which
// only pins dead code after `return` at function top level.
//
// Upstream anchors (dart-sdk reference sources):
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl: handleBreak (line 6998), handleContinue (line 7010),
//   handleExit (line 7015), and handleReturn (line 7028) all end in
//   _setCurrent(_current.setUnreachable()), so any statement after a jump in
//   the same statement sequence is unreachable.
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart: visitBreakStatement
//   (line 2285) and visitContinueStatement (line 2762) route to
//   handleBreak/handleContinue, visitReturnStatement (line 4412) routes to
//   handleReturn, and visitRethrowExpression (line 4399) routes to handleExit
//   — rethrow is a jump for flow analysis.
// - pkg/analyzer/lib/src/error/dead_code_verifier.dart,
//   NullSafetyDeadCodeVerifier: only the FIRST unreachable node is recorded
//   (_firstDead, line 533; later unreachable nodes continue the same dead
//   range, line 519), so a case body yields ONE dead_code diagnostic per dead
//   interval, not one per statement. flowEnd (line 217) anchors the report at
//   the first dead statement's first token (line 253) and trims the covering
//   SwitchMember range to its last statement (lines 276-278).
// - The interval closes at the per-member flowEnd fired by
//   handleMergedStatementCase (pkg/analyzer/lib/src/dart/resolver/
//   type_analyzer.dart, line 1277), driven after the member body statements
//   (dispatch at pkg/_fe_analyzer_shared/lib/src/type_inference/
//   type_analyzer.dart, lines 2648-2650; flow.switch_afterCase at 2653).
// - pkg/analyzer/lib/src/generated/error_verifier.dart,
//   _checkForRethrowOutsideCatch (line 7394): rethrow requires an enclosing
//   catch clause (catchClauseLevel > 0), hence the wrapping try/catch.
//
// Expected rows, approximate (the serial N2 oracle run pins exact counts):
// - dead_code x5, one per case body, each anchored at the FIRST dead
//   statement and spanning to that member's last statement:
//   1. returnInCaseBody: `print(1)` after `return`.
//   2. breakInCaseBody: `print(2)` after `break`.
//   3. continueLabelInCaseBody: `print(3)` after `continue outer`.
//   4. rethrowInCaseBody: `print(4)` after `rethrow`.
//   5. spanPinsFirstStatementOnly: two dead statements after `break` still
//      yield a single row covering both (first-statement-only anchor).
// Deliberate silences, approximate:
// - liveAfterJumpingSwitch / liveAfterRethrowingSwitch: statements after the
//   switch stay live whenever another member completes normally (the paths
//   join at flow.switch_afterCase), and a node is only recorded dead when
//   flow.isReachable is false at its own visit (dead_code_verifier.dart,
//   line 524) — the deadness above is jump-fed, not shape-fed.

// Row: dead_code at `print(1)` (flow_analysis.dart handleReturn, line 7019;
// report fired at dead_code_verifier.dart flowEnd, line 217).
void returnInCaseBody(int x) {
  switch (x) {
    case 0:
      return;
      print(1);
    default:
      break;
  }
}

// Row: dead_code at `print(2)` (flow_analysis.dart handleBreak, line 6990;
// analyzer routing visitBreakStatement, type_analyzer.dart line 2285).
void breakInCaseBody(int x) {
  switch (x) {
    case 0:
      break;
      print(2);
    default:
      break;
  }
}

// Row: dead_code at `print(3)` (flow_analysis.dart handleContinue, line 7002;
// analyzer routing visitContinueStatement, type_analyzer.dart line 2762; the
// `outer` label targets the enclosing loop, so the continue is legal).
void continueLabelInCaseBody(List<int> xs) {
  outer:
  for (var x in xs) {
    switch (x) {
      case 0:
        continue outer;
        print(3);
      default:
        break;
    }
  }
}

// Row: dead_code at `print(4)` (rethrow routes to handleExit via
// visitRethrowExpression, type_analyzer.dart line 4399, making the flow
// unreachable at flow_analysis.dart line 7015; legality: error_verifier.dart
// _checkForRethrowOutsideCatch, line 7394).
void rethrowInCaseBody(Object x) {
  try {
    throw 'x';
  } catch (e) {
    switch (x) {
      case 0:
        rethrow;
        print(4);
      default:
        print(e);
        break;
    }
  }
}

// Row: ONE dead_code spanning `print(5); print(6);`, anchored at `print(5)`
// (dead_code_verifier.dart lines 206/519/533 first-unreachable-node model,
// with the SwitchMember trim to statements.last at lines 276-278).
void spanPinsFirstStatementOnly(int x) {
  switch (x) {
    case 0:
      break;
      print(5);
      print(6);
    default:
      break;
  }
}

// Silence: no dead_code anywhere. Each jump fills its member completely, and
// `print(y)` after the switch stays reachable via the member that completes
// normally (default: break) — flow joins at switch_afterCase, and a node is
// only recorded dead when flow.isReachable is false at its own visit
// (dead_code_verifier.dart, line 524).
void liveAfterJumpingSwitch(int x, List<int> ys) {
  outer:
  for (var y in ys) {
    switch (x) {
      case 0:
        return;
      case 1:
        continue outer;
      default:
        break;
    }
    print(y);
  }
}

// Silence: no dead_code — the rethrow member is jump-exited but holds no
// trailing statement, and `print(e)` after the switch stays reachable via the
// member that completes normally.
void liveAfterRethrowingSwitch(Object x) {
  try {
    throw 'x';
  } catch (e) {
    switch (x) {
      case 0:
        rethrow;
      default:
        break;
    }
    print(e);
  }
}
