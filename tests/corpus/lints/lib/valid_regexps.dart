// valid_regexps — audit r1 thin row. An unclosed group is invalid under
// both the default and the unicode regex grammar, so no unicode: flag is
// needed (an invalid `\q` escape WITHOUT unicode:true is silent on both
// sides — that shape stays out).
// Expected rows: 1 — VALID_REGEXPS keyed valid_regexps.dart.
void f() {
  final bad = RegExp(r'([');
  print(bad);
}
