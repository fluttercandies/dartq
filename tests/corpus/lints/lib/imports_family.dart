// import family — fixture wave 2 kitchen file. Four import faces in one
// unit; every import is used so no unused_import noise rides along. The
// two dart:math prefixes partition the two prefix-name rules (the
// lowercase `_helper` satisfies library_prefixes' wildcard-tolerant
// shape, the mixed-case BadPrefix carries no leading underscore).
// Expected rows: 4, all keyed imports_family.dart — LIBRARY_PREFIXES
// (BadPrefix), NO_LEADING_UNDERSCORES_FOR_LIBRARY_PREFIXES (_helper),
// AVOID_RELATIVE_LIB_IMPORTS (the `../lib/...` URI whose path contains
// /lib/ — the written-URI face, target exists),
// DEPEND_ON_REFERENCED_PACKAGES (package:lint_corpus_nested resolves
// through the corpus root package config but is absent from this
// package's dependency set — the canonical face; its target
// nested/lib/child_library.dart exists, so the oracle's resolution-bound
// face and dartq's name-based face agree).
import 'dart:math' as BadPrefix;
import 'dart:math' as _helper;
import '../lib/imports_family_target.dart';
import 'package:lint_corpus_nested/child_library.dart';

void importUser() {
  print(BadPrefix.min(1, 2));
  print(_helper.max(1, 2));
  targetHelper();
  childHelper();
}
