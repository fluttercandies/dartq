// promotion_patterns_flow_records — interaction fixture set A (ticket
// #1425): null-safety promotion × records (positional and named fields) ×
// pattern deconstruction.
//
// Grounding (upstream Dart SDK source):
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisOperations.isPropertyPromotable (analyzer binding in
//   pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart): only
//   promotable FIELDS of classes feed property promotion. Record fields are
//   synthesized getters, never backing fields, so a record field read is
//   never promotable — a null check on it cannot feed a later read.
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowAnalysisImpl.constantPattern_end → _handleEqualityCheckPattern: a
//   `null` constant pattern is handled exactly like a null-check pattern
//   (matchesIfNull), so the matched variable is demoted to Null inside the
//   taken branch and promoted to non-Null on the fall-through.
// - pkg/analyzer/lib/src/error/nullable_dereference_verifier.dart, report:
//   when the receiver's static type is exactly Null, the dereference
//   diagnostic is swapped to invalid_use_of_null_value ("An expression whose
//   value is always 'null' can't be dereferenced"); on a merely-nullable
//   receiver in condition position the oracle reports
//   unchecked_use_of_nullable_value ("A nullable expression can't be used as
//   a condition", nail-pinned in this corpus).
// - Pattern-bound variables are fresh locals, so deconstructing a record
//   (via a record pattern or an if-case typed pattern) yields promotable
//   bindings even though the record's own fields are not promotable.
//
// Expected rows: 3 — unchecked_use_of_nullable_value keyed
// promotion_patterns_flow_records.dart (2 rows: the named field `strict` in
// condition position, and positional field $1 in condition position), and
// invalid_use_of_null_value keyed promotion_patterns_flow_records.dart (1
// row: `alias` demoted to Null by the `case null` pattern, then
// dereferenced).
//
// Deliberate silences, grounded: describe (the || demotion join promotes the
// deconstructed binding), envelope (if-case typed pattern binds a promoted
// local), gateMode (or-pattern of named record patterns sharing `level`),
// and recheckAlias (the field read is not promotable, so the guarded read
// goes through a fresh binding that does promote).
typedef RuleToggles = ({String? alias, bool? strict, int severity});

typedef Tagged = (String?, int);

// Row: unchecked_use_of_nullable_value — `config.strict` is a synthesized
// record getter; no null check can promote it, so the condition sees bool?.
String gateLabel(RuleToggles config) {
  if (config.strict) {
    return 'enforced';
  }
  return 'advisory';
}

// Row: unchecked_use_of_nullable_value — positional record field $1 has the
// same never-promotable getter face as named fields.
bool preferFastLane((bool?, int) candidate) {
  if (candidate.$1) {
    return true;
  }
  return candidate.$2 > 0;
}

// Row: invalid_use_of_null_value — the `case null` pattern demotes `alias`
// to Null inside the branch (constantPattern_end treats it as a null check),
// and a dereference of a Null-typed receiver reports
// invalid_use_of_null_value; the fall-through read below is clean because
// the same pattern promotes `alias` to String on that path.
int aliasWidth(RuleToggles config) {
  final alias = config.alias;
  if (alias case null) {
    return alias.length;
  }
  return alias.length;
}

// Silence: the deconstructed bindings are fresh locals; the `||` chain
// demotes on both disjuncts, so `alias` is a non-empty String after the if.
String describe(RuleToggles config) {
  final (:alias, :severity) = config;
  if (alias == null || alias.isEmpty) {
    return 'anon:$severity';
  }
  return '$alias:$severity';
}

// Silence: an if-case typed pattern binds `label` as String (fresh local,
// promoted by construction) even though `Tagged`'s first field is String?.
String envelope(Tagged tag) {
  final (name, weight) = tag;
  if (name case final String label when label.isNotEmpty) {
    return '$label#$weight';
  }
  return '#$weight';
}

// Silence: a logical-or of named record patterns; both sides bind `level`
// with the same type, and the wildcard arm keeps the switch exhaustive.
String gateMode(RuleToggles config) {
  return switch (config) {
    (strict: true, severity: final level) ||
    (strict: null, severity: final level) => 'hard:$level',
    _ => 'soft',
  };
}

// Silence: the field read `config.alias` never promotes, so the guard reads
// a fresh binding whose null check does promote — the fix face for the
// gateLabel shape above.
String recheckAlias(RuleToggles config) {
  if (config.alias case final String alias) {
    return alias.substring(1);
  }
  return '';
}

void main() {
  const toggles = (alias: null, strict: null, severity: 2);
  print(gateLabel(toggles));
  print(preferFastLane((true, 7)));
  print(aliasWidth((alias: 'edge', strict: null, severity: 3)));
  print(describe(toggles));
  print(envelope((null, 5)));
  print(gateMode(toggles));
  print(recheckAlias((alias: 'mirror', strict: true, severity: 1)));
}
