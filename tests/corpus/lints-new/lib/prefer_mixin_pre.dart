// prefer_mixin — differential corpus (#1160), LANGUAGE-2.19 face.
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/prefer_mixin_test.dart, PreferMixinTestLanguage219
// (transcribed 2026-10-14, vendored SDK 3.15 main face). The `// @dart =
// 2.19` comment pins this library to the pre-mixin-class language, where
// `with` a plain class lints instead of erroring — the twin of
// prefer_mixin.dart (a @dart comment governs the whole file, so the faces
// cannot share a library). Whole-file imports (upstream declares these per
// case; hoisted because a merged corpus file keeps its directives at the
// top).
// Expected rows (oracle leg): 2 — PREFER_MIXIN keyed
// prefer_mixin_pre.dart, one per POSITIVE block.

// @dart = 2.19

import 'dart:collection';
import 'dart:convert';

// POSITIVE upstream test_mixedInClass (1 row at the mixed-in class name).
class M19Plain {}

class M19User extends Object with M19Plain {}

// POSITIVE upstream test_mixedInClass_typAlias (1 row at the alias name).
class M19AliasTarget {}

typedef M19Alias = M19AliasTarget;

abstract class M19AliasUser with M19Alias {}

// NEGATIVE upstream test_mixedInIterableMixin (re-tested per core mixin at
// 2.19, mirroring the Language219 suite).
abstract class M19Iterable with IterableMixin {}

// NEGATIVE upstream test_mixedInListMixin.
abstract class M19List with ListMixin {}

// NEGATIVE upstream test_mixedInMaoMixin (upstream's own spelling of the
// Map face).
abstract class M19Map with MapMixin {}

// NEGATIVE upstream test_mixedInMixin.
mixin M19Mixin {}

class M19MixinUser with M19Mixin {}

// NEGATIVE upstream test_mixedInSetMixin.
abstract class M19Set with SetMixin {}

// NEGATIVE upstream test_mixedInStringConversionSinkMixin.
abstract class M19Sink with StringConversionSinkMixin {}
