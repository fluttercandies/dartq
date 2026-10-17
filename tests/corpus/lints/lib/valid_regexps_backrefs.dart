// valid_regexps — backreference faces. The VM's backref validity rule
// (upstream Dart SDK source, runtime/vm/regexp/regexp-parser.cc,
// ParseBackReferenceIndex): a `\N` escape parses as a backreference only
// when N is no greater than the pattern's TOTAL capture-group count — the
// parser first compares against the groups started so far and, when N is
// larger, scans the whole pattern for the total (`capture_count_`) before
// deciding. Forward references therefore parse (`\1(a)`), and an index
// beyond the total is a parse-time failure — `RegExp(r'\2(a)')` throws at
// runtime, so the rule reports. Each shape rides its own `RegExp`
// creation with `unicode: true`.
// Expected rows: 2 — VALID_REGEXPS keyed valid_regexps_backrefs.dart:
//   `\2(a)` (total captures 1 < 2) and `(a)\2` (total captures 1 < 2).
void forwardRefBeforeItsGroup() {
  final ok = RegExp(r'\1(a)', unicode: true);
  print(ok);
}

void nestedOuterBackref() {
  final ok = RegExp(r'((a))\1', unicode: true);
  print(ok);
}

void namedGroupsBackref() {
  final ok = RegExp(r'(?<a>(?<b>c))\1', unicode: true);
  print(ok);
}

void indexBeyondTotalBefore() {
  final bad = RegExp(r'\2(a)', unicode: true);
  print(bad);
}

void indexBeyondTotalAfter() {
  final bad = RegExp(r'(a)\2', unicode: true);
  print(bad);
}
