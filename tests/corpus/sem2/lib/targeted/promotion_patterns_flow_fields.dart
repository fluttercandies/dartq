// promotion_patterns_flow_fields — interaction fixture set A (ticket #1425):
// field promotion (private final faces, late final faces, non-final faces) ×
// patterns × flow, including a this-reachable field read after pattern
// deconstruction.
//
// Grounding (upstream Dart SDK source):
// - pkg/_fe_analyzer_shared/lib/src/field_promotability.dart, addField: a
//   field is promotable only when it is private, final, and not external,
//   with no conflicting non-final field, concrete getter, or noSuchMethod
//   forwarder of the same name in the library — eligibility never consults
//   `late`.
// - pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart,
//   FlowModel.write: an assignment to a variable demotes its promotions at
//   the write point (the documented non-promotion reason "write"), which is
//   how the promoted parameter face below loses `strict != null`.
// - pkg/analyzer/lib/src/error/bool_expression_verifier.dart,
//   checkForNonBoolExpression: a bool?-typed operand in a condition position
//   reports unchecked_use_of_nullable_value (3.13.1 oracle message
//   "A nullable expression can't be used as a condition.", nail-pinned in
//   this corpus).
// - Pattern deconstruction creates fresh local bindings and does not write
//   instance fields, so a this-reachable private final field read after a
//   record deconstruction still carries its promotion.
//
// Expected rows: 3 — unchecked_use_of_nullable_value keyed
// promotion_patterns_flow_fields.dart (3 rows: the `while` condition on the
// never-promotable non-final field, the `if` condition on the same field,
// and the condition on the `strict` parameter after the demoting write).
//
// Deliberate silences, grounded: render (if-case typed pattern over the
// promotable private final field), annotateFormula (the promotion survives
// the record deconstruction because the deconstruction writes no fields).
//
// Unarbitrated in this file: armStatus and cachedLength exercise `late final`
// private field reads (one with initializer, one assigned once via seed()).
// The eligibility rule above does not exclude late fields at this SDK
// revision, but the release oracle caliber's behavior on them is not pinned
// here — these two faces are differential probes only, and the serial oracle
// run arbitrates whether they stay silent or emit (an emission would be
// unchecked_use_of_nullable_value on the `_armed` condition).
class ReportBuilder {
  // Private final: promotable (field_promotability.dart addField).
  final String? _annotation;

  // Private non-final: never promotable — every read sees bool?.
  bool? _strict;

  // Late final with initializer: probe face, see the header note.
  late final bool? _armed = null;

  // Late final without initializer, assigned once via seed(): probe face.
  late final String? _cache;

  final (String, String) _formula = ('sum', '42');

  ReportBuilder(this._annotation);

  ReportBuilder seed(String? value) {
    _cache = value;
    return this;
  }

  // Silence: if-case typed pattern over a promotable private final field;
  // `note` is a fresh String binding, the guard refines it further.
  String render() {
    if (_annotation case final String note when note.isNotEmpty) {
      return note.toUpperCase();
    }
    return 'blank';
  }

  // Silence: the record deconstruction only creates locals; the subsequent
  // null check still promotes the this-reachable final field `_annotation`,
  // so the interpolated read needs no re-check.
  String annotateFormula() {
    final (kind, operand) = _formula;
    if (_annotation != null) {
      return '$kind($operand): ${_annotation.length}';
    }
    return '$kind($operand)';
  }

  // Row: unchecked_use_of_nullable_value (the `while` condition) — `_strict`
  // is non-final, hence never promotable; the condition sees bool?.
  void enforce() {
    var attempts = 0;
    while (_strict && attempts < 3) {
      attempts++;
      if (attempts == 2) {
        _strict = false;
      }
    }
  }

  // Row: unchecked_use_of_nullable_value — same never-promotable field, `if`
  // condition position.
  bool get hardened {
    if (_strict) {
      return true;
    }
    return false;
  }

  // Row: unchecked_use_of_nullable_value — `strict` was promoted by the null
  // check, but the assignment from `fallback` (bool?) demotes it at the
  // write point, so the guarded condition sees bool? again.
  String resolve(bool? strict, bool? fallback) {
    if (strict != null) {
      strict = fallback;
      if (strict) {
        return 'strict';
      }
    }
    return 'lenient';
  }

  // Unarbitrated probe: if `late final` fields promote at the release oracle
  // caliber, both conjuncts are clean; if not, the right operand is a
  // bool?-typed condition and reports unchecked_use_of_nullable_value.
  String armStatus() {
    if (_armed != null && _armed) {
      return 'armed';
    }
    return 'idle';
  }

  // Unarbitrated probe: the typed pattern binds `cached` as a fresh String
  // local regardless of whether the late final field itself promotes, so
  // this face stays silent either way and only exercises the read shape.
  int? cachedLength() {
    if (_cache case final String cached) {
      return cached.length;
    }
    return null;
  }
}

void main() {
  var builder = ReportBuilder('audit note');
  print(builder.render());
  print(builder.annotateFormula());
  builder.enforce();
  print(builder.hardened);
  print(builder.resolve(true, null));
  print(builder.seed('cache').cachedLength());
  print(builder.armStatus());
}
