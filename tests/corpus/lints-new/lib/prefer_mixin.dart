// prefer_mixin — differential corpus (#1160), 3.13 face.
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/prefer_mixin_test.dart (transcribed 2026-10-14,
// vendored SDK 3.15 main face).
// This file carries the language-ungated NEGATIVE faces at the corpus
// language version 3.13 (where `with` a plain class is a compile error and
// `mixin class` is legal, so only genuine mixins may be mixed in). The two
// POSITIVE faces (upstream PreferMixinTestLanguage219) need the library at
// language 2.19 and live in prefer_mixin_pre.dart — a @dart comment
// governs the whole file, so the faces cannot share a library.
// Expected rows (oracle leg): 0 — PREFER_MIXIN keyed prefer_mixin.dart.

// NEGATIVE upstream test_legacyCoreClasses (the core library mixins are
// exempt on every face).
import 'dart:collection';
import 'dart:convert';

abstract class CLegacy with IterableMixin {}

abstract class CLegacyList with ListMixin {}

abstract class CLegacyMap with MapMixin {}

abstract class CLegacySet with SetMixin {}

abstract class CLegacySink with StringConversionSinkMixin {}

// NEGATIVE upstream test_mixedInMixin_ok.
mixin MPlain {}

class CMixin with MPlain {}

// NEGATIVE upstream test_mixedInTypeAlias_ok.
typedef TAliasPlain = MPlain;

abstract class CCTypeAlias with TAliasPlain {}

// NEGATIVE upstream test_mixinClass (issue 4065: a `mixin class` mixes in
// freely at 3.13).
mixin class MMixinClass {}

class CZMixinClass with MMixinClass {}
