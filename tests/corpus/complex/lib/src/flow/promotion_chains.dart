/// Flow-sensitive type promotion in the shapes real services use: is/is!
/// demotion ladders, && and || promotion joins, ?? chains, if-case
/// promotion and private-final field promotion.
library;

import '../core/identifiers.dart';

/// Length of a possibly-absent label; is! + early return demotes the null
/// and non-String arms in one step.
int labelLength(Object? label) {
  if (label is! String) {
    return 0;
  }
  return label.length;
}

/// && joins promotions: after the second conjunct, [raw] is String and
/// non-empty in the body.
String firstToken(String? raw) {
  if (raw != null && raw.isNotEmpty) {
    return raw.substring(0, 1);
  }
  return '';
}

/// || demotion: both disjuncts reject the bad shapes, so the body sees a
/// promoted [raw].
String normalizedPort(String? raw) {
  if (raw == null || raw.isEmpty || raw.length > 5) {
    return '0';
  }
  return raw.padLeft(5, '0');
}

/// ?? chains collapse absent values without re-checking.
String effectiveNickname(String? nick, String? handle, String fallback) =>
    nick ?? handle ?? fallback;

/// if-case binding: the matched variable is String inside the branch.
String describePayload(Object? payload) {
  if (payload case final String text) {
    return 'text of ${text.length}';
  }
  if (payload case List<Object?> items when items.length > 3) {
    return 'long list of ${items.length}';
  }
  return 'opaque payload';
}

/// Field promotion: [_annotation] is a private final nullable field, so the
/// null check promotes it for the whole guarded block; [_reviewer] shows the
/// same promotion reused through a chain of && conjuncts.
class AuditStamp {
  final String? _annotation;
  final String? _reviewer;
  final CustomerRef subject;

  AuditStamp(this.subject, this._annotation, this._reviewer);

  bool get hasAnnotation {
    if (_annotation != null && _reviewer != null) {
      return _reviewer.isNotEmpty;
    }
    return false;
  }

  /// Promotion survives into a closure only for final locals; the field is
  /// copied to a final local first, the standard pattern.
  String renderAnnotation() {
    final annotation = _annotation;
    if (annotation == null) {
      return 'no annotation for ${subject.value}';
    }
    return () {
      return 'note: $annotation';
    }();
  }
}

/// Promotion through a catch clause: the on-clause types [fault] as
/// FormatException for the whole handler body.
String explainParse(String raw) {
  try {
    final value = int.parse(raw);
    return 'parsed $value';
  } on FormatException catch (fault) {
    return 'bad format: ${fault.message}';
  }
}
