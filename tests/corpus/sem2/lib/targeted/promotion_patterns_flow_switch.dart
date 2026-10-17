// promotion_patterns_flow_switch — interaction fixture set A (ticket #1425):
// property promotion × patterns × flow across switch expressions and switch
// statements. Companion to the single-code targeted fixtures; the clean
// faces here are near-miss shapes whose silence is load-bearing.
//
// Grounding (upstream Dart SDK source):
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl._getMatchedValueType: the type a case pattern is checked
//   against is the scrutinee promotion key's promotedTypes.lastOrNull, so an
//   `is!` + early return feeds the promoted type to every arm below.
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl.logicalOrPattern_end: a logical-or pattern's shared
//   variables must bind on both sides with the same type; the matched state
//   joins both disjuncts' demotions.
// - Exhaustiveness (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
//   resolver/type_analyzer.dart, analyzeSwitchExpression): a case arm carrying
//   a `when` guard contributes nothing to the covered space (the guard cannot
//   be proven true statically), so the arm is excluded from the model; a
//   switch expression over a sealed type that leaves space uncovered reports
//   non_exhaustive_switch_expression.
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
//   visitSwitchStatement: a statement switch may complete normally when no
//   member matches, so the enclosing body can complete normally afterwards
//   (body_might_complete_normally on the function body).
// - pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart,
//   visitConstantPattern: a constant-pattern expression that is not a valid
//   constant (here: a local variable reference, however final or promoted)
//   reports constant_pattern_with_non_constant_expression.
//
// Expected rows: 4 — non_exhaustive_switch_expression keyed
// promotion_patterns_flow_switch.dart (2 rows: the or-pattern arm set that
// misses Empty, and the guarded Binary arm that covers nothing),
// body_might_complete_normally keyed promotion_patterns_flow_switch.dart
// (1 row: the statement switch that Empty falls through), and
// constant_pattern_with_non_constant_expression keyed
// promotion_patterns_flow_switch.dart (1 row: the promoted local in constant
// position).
//
// Deliberate silences, grounded: encodedSize (promoted scrutinee feeds the
// matched value type), packedSize (an exhaustive logical-or of object
// patterns over the full sealed set), and tier (a promotion held before a
// statement switch survives past it when no arm writes the variable — arms
// join at flow.switch_afterCase without demotion).
sealed class Payload {
  const Payload();
}

class Text extends Payload {
  final int chars;

  const Text(this.chars);
}

class Binary extends Payload {
  final int bytes;

  const Binary(this.bytes);
}

class Empty extends Payload {
  const Empty();
}

// Silence: the `is!` + return promotes `value` to Payload, and the promoted
// type is the matched value type every object pattern below is checked
// against (flow_analysis.dart _getMatchedValueType).
int encodedSize(Object value) {
  if (value is! Payload) {
    return 0;
  }
  return switch (value) {
    Text(chars: final chars) => chars,
    Binary(bytes: final bytes) => bytes,
    Empty() => 0,
  };
}

// Silence: the logical-or arm binds `size` on both sides with the same type
// (int), and together with the Empty arm the sealed set is fully covered.
int packedSize(Payload payload) {
  return switch (payload) {
    Text(chars: final size) || Binary(bytes: final size) => size,
    Empty() => 0,
  };
}

// Row: non_exhaustive_switch_expression — the or-pattern covers Text and
// Binary but the sealed hierarchy's Empty is uncovered, so the switch
// expression is not exhaustive.
int orphanedSize(Payload payload) {
  return switch (payload) {
    Text(chars: final size) || Binary(bytes: final size) => size,
  };
}

// Row: non_exhaustive_switch_expression — the `when size > 512` guard
// excludes the Binary arm from the exhaustiveness model entirely, leaving
// Binary(bytes <= 512) unmatched even though every subtype is named.
int guardedSize(Payload payload) {
  return switch (payload) {
    Text(chars: final size) => size,
    Binary(bytes: final size) when size > 512 => size,
    Empty() => 0,
  };
}

// Row: body_might_complete_normally — a statement switch whose members do
// not cover the sealed set completes normally when an Empty payload arrives,
// and the body then falls off the end of a non-void function.
String label(Payload payload) {
  switch (payload) {
    case Text():
      return 'text';
    case Binary():
      return 'binary';
  }
}

// Row: constant_pattern_with_non_constant_expression — `marker` is a
// promoted non-null local, but promotion does not make it a constant; a
// constant pattern must be a constant expression.
int score(String? marker, String raw) {
  if (marker == null) {
    return 0;
  }
  return switch (raw) {
    marker => 100,
    _ => 1,
  };
}

// Silence: `value` is promoted to String before the statement switch, no arm
// writes it, and the arms join back at flow.switch_afterCase — the promotion
// stands for the substring read after the switch.
String tier(Object value) {
  if (value is! String) {
    return 'n/a';
  }
  switch (value) {
    case 'alpha':
      break;
    default:
      break;
  }
  return value.substring(1);
}

void main() {
  print(encodedSize(Text(3)));
  print(packedSize(const Binary(9)));
  print(tier('alpha'));
}
