// prefer_const_literals_to_create_immutables — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/prefer_const_literals_to_create_immutables_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 12 POSITIVE + 9 NEGATIVE = all 21 upstream tests, none
// skipped. The @immutable/@Immutable faces resolve through the corpus's
// vendored package:meta (vendor/meta), the minimal stand-in for upstream's
// addMetaPackageDep. The error-only cases (missingRequiredArgument,
// newWithNonType) are NEGATIVE for the lint face.
// Expected rows (oracle leg): 13 —
// PREFER_CONST_LITERALS_TO_CREATE_IMMUTABLES keyed
// prefer_const_literals_to_create_immutables.dart (site counts in block
// headers).

import 'package:meta/meta.dart';

// NEGATIVE upstream test_boolFromEnvironment: a collection-if whose
// condition is not compile-time constant skips the rule.
const xBoolFromEnv = bool.fromEnvironment('dart.library.js_util');

@immutable
class ABoolFromEnv {
  const ABoolFromEnv(List<int> p);
}

var aBoolFromEnv = ABoolFromEnv([
  if (xBoolFromEnv) eBoolFromEnv(),
]);

int eBoolFromEnv() => 7;

// POSITIVE upstream test_extensionType (1 row at `[1]`; official behavior
// transcribed verbatim — the representation constructor of the extension
// type is the invocation target).
@immutable
extension type EList(List<int> i) { }

var eList = EList([1]);

// NEGATIVE upstream test_listLiteral_const.
@immutable
class CListConst {
  const CListConst(List<Object> p);
}
var xListConst = CListConst(const []);

// NEGATIVE upstream test_listLiteral_nested_const.
@immutable
class CListNestedConst {
  const CListNestedConst(List<Object> p);
}
var xListNestedConst = CListNestedConst(const [
  const [],
]);

// POSITIVE upstream test_listLiteral_nested_noConst (2 rows: the outer list
// and the inner list).
@immutable
class CListNestedNoConst {
  const CListNestedNoConst(List<Object> p);
}
var xListNestedNoConst = CListNestedNoConst([
  [],
]);

// POSITIVE upstream test_listLiteral_noConst (1 row at `[]`).
@immutable
class CListNoConst {
  const CListNoConst(List<Object> p);
}
var xListNoConst = CListNoConst([]);

// POSITIVE upstream test_listLiteral_noConst_instantiationAnnotation (1 row
// at `[]`; the capitalized @Immutable spelling resolves to the same
// annotation constructor).
@Immutable('')
class CListCapAnnotation {
  const CListCapAnnotation(List<Object> p);
}
var xListCapAnnotation = CListCapAnnotation([]);

// NEGATIVE upstream test_listLiteral_notConstable_noConst: a non-const
// element (`D()`) makes the literal non-constable.
@immutable
class CListNotConstable {
  const CListNotConstable(List<Object> p);
}
class DNotConstable {}
var xListNotConstable = CListNotConstable([DNotConstable()]);

// NEGATIVE upstream test_mapLiteral_const.
@immutable
class CMapConst {
  const CMapConst(Map<int, int> p);
}
var xMapConst = CMapConst(const {1: 2});

// NEGATIVE upstream test_mapLiteral_inParens_const.
@immutable
class CMapParensConst {
  const CMapParensConst(Object? p);
}
var xMapParensConst = CMapParensConst(((const {})));

// POSITIVE upstream test_mapLiteral_inParens_noConst (1 row at `{}`).
@immutable
class CMapParensNoConst {
  const CMapParensNoConst(Object? p);
}
var xMapParensNoConst = CMapParensNoConst((({})));

// POSITIVE upstream test_mapLiteral_intToDouble_noConst (1 row).
@immutable
class CMapIntDouble {
  const CMapIntDouble(Map<int, Object?> p);
}
var xMapIntDouble = CMapIntDouble({1: 1.0});

// POSITIVE upstream test_mapLiteral_intToInstantiation_const (1 row).
@immutable
class CMapInstantiation {
  const CMapInstantiation(Object? p);
}
var xMapInstantiation = CMapInstantiation({1: const CMapInstantiation(null)});

// POSITIVE upstream test_mapLiteral_intToInt_noConst (1 row).
@immutable
class CMapIntInt {
  const CMapIntInt(Map<int, Object?> p);
}
var xMapIntInt = CMapIntInt({1: 1});

// POSITIVE upstream test_mapLiteral_intToNull_noConst (1 row).
@immutable
class CMapIntNull {
  const CMapIntNull(Map<int, Object?> p);
}
var xMapIntNull = CMapIntNull({1: null});

// POSITIVE upstream test_mapLiteral_intToString_noConst (1 row).
@immutable
class CMapIntString {
  const CMapIntString(Map<int, Object?> p);
}
var xMapIntString = CMapIntString({1: ''});

// POSITIVE upstream test_mapLiteral_noConst (1 row).
@immutable
class CMapNoConst {
  const CMapNoConst(Map<int, int> p);
}
var xMapNoConst = CMapNoConst({1: 2});

// NEGATIVE upstream test_missingRequiredArgument (no lint; the missing
// required argument error is outside the lint filter).
@immutable
class KMissing {
  final List<KMissing> children;
  const KMissing({required this.children});
}

final kMissing = KMissing(
  children: <KMissing>[for (var i = 0; i < 5; ++i) KMissing()],
);

// POSITIVE upstream test_namedParameter_noConst (1 row at `[]`).
@immutable
class CNamedParam {
  const CNamedParam({Object? p});
}
var xNamedParam = CNamedParam(p: []);

// NEGATIVE upstream test_newWithNonType (no lint; `B` is undefined here, so
// the newWithNonType error is outside the lint filter).
var e1NewWithNonType = new B([]);

// NEGATIVE upstream test_notImmutable_noConst: the constructed class is not
// annotated @immutable.
class CNotImmutable {
  const CNotImmutable(List<Object> p);
}
var xNotImmutable = CNotImmutable([]);
