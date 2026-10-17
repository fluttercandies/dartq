// Targets constant_pattern_never_matches_value_type fed by a PROMOTED switch
// scrutinee (ticket #1376, corpus gap audit face F2): a case constant is
// type-checked against the flow-analysis matched value type, which is the
// scrutinee's promoted type when promotion exists, never the declared type.
//
// Upstream anchors (dart-sdk reference sources):
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl._getMatchedValueType: the matched value type used to type
//   check a pattern is the scrutinee promotion key's promotedTypes.lastOrNull,
//   falling back to the declared type only when no promotion exists.
// - pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
//   ConstantVerifier.visitConstantPattern: reports
//   constant_pattern_never_matches_value_type when _canBeEqual(constantType,
//   matchedValueType) is false, for constants with primitive equality.
// - pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
//   ConstantVerifier._canBeEqual: InterfaceType probes constantType <:
//   greatestClosure(valueType); a type-parameter matched value recurses into
//   promotedBound ?? bound (the T & int intersection path).
// - pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart,
//   constantPatternNeverMatchesValueType: DiagnosticType.STATIC_WARNING.
// - pkg/analyzer/lib/src/dart/constant/value.dart: IntState, StringState, and
//   NullState.hasPrimitiveEquality are all true, so int, String, and null
//   constants participate in the check.
//
// Expected rows, approximate (the serial N2 oracle run pins exact counts):
// - constant_pattern_never_matches_value_type x4:
//   1. isPromotedStatementSwitch `case 'x'`: String constant vs Object
//      promoted to int by `x is int`.
//   2. nullPromotedSwitch `case null`: null constant vs String? promoted to
//      String by the `s != null` check.
//   3. intersectionPromotedSwitch `case 'x'`: String constant vs the T & int
//      intersection matched type.
//   4. switchExpressionAnchor `'x'`: same promotion in switch expression
//      position, pinning statement/expression agreement.
// Deliberate silences, approximate:
// - isPromotedStatementSwitch `case 3`: int constant vs promoted int.
// - nullPromotedSwitch `case ''`: String constant vs promoted String.
// - unpromotedControl: the same constants as face 1 against a declared Object
//   scrutinee with no promotion stay silent because String <: Object, proving
//   the behavior is promotion-fed rather than shape-fed.
void isPromotedStatementSwitch(Object x) {
  if (x is int) {
    switch (x) {
      case 'x':
        break;
      case 3:
        break;
    }
  }
}

void nullPromotedSwitch(String? p) {
  String? s = p;
  if (s != null) {
    switch (s) {
      case null:
        break;
      case '':
        break;
    }
  }
}

void intersectionPromotedSwitch<T extends num>(T t) {
  if (t is int) {
    switch (t) {
      case 'x':
        break;
      case 2:
        break;
    }
  }
}

int switchExpressionAnchor(Object x) {
  if (x is int) {
    return switch (x) {
      'x' => 1,
      _ => 0,
    };
  }
  return 0;
}

void unpromotedControl(Object x) {
  switch (x) {
    case 'x':
      break;
    case 3:
      break;
  }
}
