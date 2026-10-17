// promotion_patterns_flow_tryfinally — interaction fixture set A (ticket
// #1425): definite-assignment analysis × exception flow × labeled jumps, with
// promotion held across try/finally and labeled continue/break.
//
// Grounding (upstream Dart SDK source):
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl.tryFinallyStatement_end: with no catch clause, the state
//   after a try/finally is the try body's normal-completion state with the
//   finally's effects attached (the exception path exits the function), so a
//   variable assigned unconditionally in the try body is definitely assigned
//   after the statement.
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl.tryFinallyStatement_finallyBegin: entering the finally,
//   only variables WRITTEN or CAPTURED in the try body are conservatively
//   demoted — an untouched promotion survives into the finally and past the
//   statement.
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
//   _checkReadOfNotAssignedLocalVariable: a late local read while definitely
//   unassigned reports definitely_unassigned_late_local_variable; a non-late
//   final local read while not definitely assigned reports
//   read_potentially_unassigned_final; a non-late non-final local of
//   potentially non-nullable type read while not definitely assigned reports
//   not_assigned_potentially_non_nullable_local_variable (all three
//   COMPILE_TIME_ERROR).
// - Labeled continue/break are plain jumps for flow analysis
//   (handleContinue/handleBreak): they neither write nor demote variables
//   they do not assign, so a promotion held before the loop survives a
//   back-edge join that does not write it.
//
// Expected rows: 3 — definitely_unassigned_late_local_variable keyed
// promotion_patterns_flow_tryfinally.dart (1 row: the late digest read
// before any path assigns it), read_potentially_unassigned_final keyed
// promotion_patterns_flow_tryfinally.dart (1 row: the settled read after a
// try/catch whose handler leaves it unassigned), and
// not_assigned_potentially_non_nullable_local_variable keyed
// promotion_patterns_flow_tryfinally.dart (1 row: the port read after a
// labeled loop that may exit without running its body).
//
// Gate tier note: the three definite-assignment codes above sit outside the
// frozen sem2 code universe (the gate's codes file filters them out); they
// are grounded anchors for full-plane differential runs.
//
// Deliberate silences, grounded: rebuild (try/finally definite assignment),
// holdAcrossFinally (promotion survives try/finally for an unwritten final
// field), compactLength (promotion survives labeled continue/break), and
// firstOpen (a while(true) loop whose every exit assigns first gives the
// definite-assignment guarantee the port face lacks).
void trace(String message) {
  print(message);
}

class StatusLine {
  final String? _status;

  StatusLine(this._status);

  // Row: definitely_unassigned_late_local_variable — `digest` is read inside
  // the try before the only assignment on any path has executed.
  String seal(String raw) {
    late String digest;
    try {
      trace('sealing ${digest.length}');
      digest = raw.trim();
      return digest;
    } finally {
      trace('sealed');
    }
  }

  // Silence: `settled` assigned unconditionally in the try, read after the
  // try/finally — the exception path leaves the function, so the read is
  // definitely assigned (tryFinallyStatement_end).
  String rebuild(String raw) {
    late final String settled;
    try {
      settled = raw.trim();
    } finally {
      trace('rebuild done');
    }
    return settled;
  }

  // Row: read_potentially_unassigned_final — the catch path completes
  // normally without assigning `settled`, so the trailing read is only
  // potentially assigned.
  int settle(String raw) {
    final int settled;
    try {
      settled = int.parse(raw);
    } catch (error) {
      trace('$error');
    }
    return settled;
  }

  // Silence: `_status` is a private final field promoted by the null check;
  // the try body and finally never write or capture it, so the promotion
  // holds inside the body, inside the finally, and after the statement
  // (tryFinallyStatement_finallyBegin demotes only written/captured
  // variables).
  String render() {
    if (_status == null) {
      return 'idle';
    }
    try {
      trace(_status.toUpperCase());
    } finally {
      trace('status=$_status');
    }
    return _status.trim();
  }
}

// Row: not_assigned_potentially_non_nullable_local_variable — the loop body
// may run zero times (or never hit the prefix), leaving `port` unassigned on
// the loop's normal-completion path that reaches the return.
int chosenPort(List<String> nodes) {
  int port;
  outer:
  for (final node in nodes) {
    if (node.startsWith('edge:')) {
      port = 443;
      break outer;
    }
  }
  return port;
}

// Silence: while(true) never completes normally, and every break is preceded
// by an assignment to `open`, so the read after the loop is definitely
// assigned — the guarantee the chosenPort face lacks.
int firstOpen(List<int> probes) {
  var index = 0;
  int open;
  outer:
  while (true) {
    open = probes[index];
    if (open > 0) {
      break outer;
    }
    index++;
    if (index == probes.length) {
      open = -1;
      break outer;
    }
  }
  return open;
}

// Silence: `head` is promoted to String before the loop; the labeled
// continue and break jump without writing it, so the promotion survives the
// back-edge join and the post-loop read needs no re-check.
int compactLength(List<String?> cells) {
  if (cells.isEmpty) {
    return 0;
  }
  final head = cells.first;
  if (head == null) {
    return 0;
  }
  outer:
  for (var i = 0; i < cells.length; i++) {
    final cell = cells[i];
    if (cell == null) {
      continue outer;
    }
    if (cell.isEmpty) {
      break outer;
    }
  }
  return head.length;
}

void main() {
  var line = StatusLine(' ready ');
  print(line.rebuild(' draft '));
  print(line.render());
  print(firstOpen([0, 3]));
  print(compactLength(['a', null, 'b']));
}
