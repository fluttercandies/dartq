// prefer_const_declarations — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/prefer_const_declarations_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 23 POSITIVE + 23 NEGATIVE + 1 SKIPPED = all 47 upstream
// tests. SKIPPED (1): test_constructorTearoff_instantiatedWithTypeVariable
// — upstream marks it @FailingTest (issue 2911); transcribing it would
// score the oracle's known failure as a differential verdict. The two
// identical upstream blocks test_staticField_final_listLiteral and
// test_staticField_final_listLiteral_const are transcribed ONCE (same
// source, one block — the dedupe is noted inline), so this file carries
// 22 POSITIVE blocks. Upstream cases each compiled standalone;
// declarations carry block-suffixed names.
// Expected rows (oracle leg): 22 — PREFER_CONST_DECLARATIONS keyed
// prefer_const_declarations.dart, one per POSITIVE block.

// POSITIVE upstream test_constructorTearoff_inference (1 row at `c`).
void fCtorTearoffInference() {
  final CInference<int> Function() c = CInference.new;
}

class CInference<T> {}

// POSITIVE upstream test_constructorTearoff_instantiatedWithType (1 row at
// `c`).
void fCtorTearoffInstantiated() {
  final c = CInstantiated<int>.new;
}

class CInstantiated<T> {}

// POSITIVE upstream test_constructorTearoff_uninstantiated (1 row at `x`).
void fCtorTearoffUninstantiated() {
  final x = CUninstantiated.new;
}

class CUninstantiated<T> {}

// NEGATIVE upstream test_instanceField_final_listLiteral_const.
class CFieldConst {
  final x = const [];
}

// NEGATIVE upstream test_localVariable_const_listLiteral_const.
void fLocalConstConst() {
  const x = const [];
}

// POSITIVE upstream test_localVariable_final_constructorInvocation_const
// (1 row at `x`).
void fFinalConstCtor() {
  final x = const CConstCtor();
}

class CConstCtor {
  const CConstCtor();
}

// NEGATIVE upstream test_localVariable_final_constructorInvocation_new.
void fFinalNonConstCtor() {
  final x = CConstCtor();
}

// POSITIVE upstream test_localVariable_final_doubleLiteral (1 row at `x`).
void fFinalDouble() {
  final x = 1.3;
}

// POSITIVE upstream test_localVariable_final_intLiteral (1 row at `x`).
void fFinalInt() {
  final x = 1;
}

// NEGATIVE upstream test_localVariable_final_intLiteral_multiple
// (a multi-variable declaration is never rewritten — SDK issue 32745).
void fFinalMultiple() {
  final x, y = 1;
}

// NEGATIVE upstream test_localVariable_final_listLiteral (an unconst-able
// list literal keeps `final`).
void fFinalList() {
  final x = [];
}

// POSITIVE upstream test_localVariable_final_listLiteral_const (1 row at
// `x`).
void fFinalListConst() {
  final x = const [];
}

// POSITIVE upstream test_localVariable_final_mapOrSetLiteral_const (1 row
// at `x`).
void fFinalMapOrSetConst() {
  final x = const {};
}

// NEGATIVE upstream test_localVariable_final_methodInvocation.
void fFinalMethodInvocation() {
  final x = 7.toString();
}

// POSITIVE upstream test_localVariable_final_nullLiteral (1 row at `x`).
void fFinalNull() {
  final x = null;
}

// NEGATIVE upstream test_localVariable_final_prefixedIdentifier.
void fFinalPrefixedIdentifier() {
  final x = 7.hashCode;
}

// POSITIVE upstream test_localVariable_final_staticProperty_const (1 row
// at `x`).
void fFinalStaticProperty() {
  final x = CStaticProp.p;
}

class CStaticProp {
  const CStaticProp();
  static const p = const [];
}

// POSITIVE upstream test_localVariable_final_stringLiteral (1 row at `x`).
void fFinalString() {
  final x = '';
}

// NEGATIVE upstream test_localVariable_listLiteral_final_typed (an explicit
// type defeats the inference face).
void fTypedList() {
  final List<int> x = [];
}

// NEGATIVE upstream test_localVariable_listLiteral_final_typed_typeArg.
void fTypedListTypeArg() {
  final List<int> x = <int>[];
}

// NEGATIVE upstream test_localVariable_listLiteral_final_untyped_typeArg.
void fUntypedListTypeArg() {
  final x = <int>[];
}

// NEGATIVE upstream test_localVariable_mapLiteral_final_typed.
void fTypedMap() {
  final Map<int, int> x = {};
}

// NEGATIVE upstream test_localVariable_mapLiteral_final_typed_typeArgs.
void fTypedMapTypeArgs() {
  final Map<int, int> x = <int, int>{};
}

// NEGATIVE upstream test_localVariable_mapLiteral_final_untyped_typeArgs.
void fUntypedMapTypeArgs() {
  final x = <int, int>{};
}

// NEGATIVE upstream test_localVariable_mapOrSetLiteral_final_untyped.
void fUntypedMapOrSet() {
  final x = {};
}

// NEGATIVE upstream test_localVariable_setLiteral_final_typed.
void fTypedSet() {
  final Set<int> x = {};
}

// NEGATIVE upstream test_localVariable_setLiteral_final_typed_typeArg.
void fTypedSetTypeArg() {
  final Set<int> ids = <int>{};
}

// POSITIVE upstream test_recordLiteral (1 row at `tuple`).
final tupleConst = const ('first', 2, true);

// NEGATIVE upstream test_staticField_const_listLiteral_const.
class CStaticFieldConst {
  static const x = const [];
}

// POSITIVE upstream test_staticField_final_listLiteral AND
// test_staticField_final_listLiteral_const — the two upstream blocks are
// byte-identical (the upstream file carries the same source twice);
// transcribed once (1 row at `x`).
class CStaticFieldFinal {
  static final x = const [];
}

// POSITIVE upstream test_staticField_final_nullLiteral (1 row at `x`).
class CStaticFieldNull {
  static final x = null;
}

// POSITIVE upstream test_staticFunctionTearoff_inference (1 row at `c`).
void fStaticTearoffInference() {
  final CStaticTear<int> Function() c = CStaticTear.m;
}

class CStaticTear<T> {
  static C<X> m<X>() => C<X>();
}

// POSITIVE upstream test_staticFunctionTearoff_instantiatedWithType (1 row
// at `c`).
void fStaticTearoffInstantiated() {
  final c = CStaticTearInst.m<int>;
}

class CStaticTearInst<T> {
  static C<X> m<X>() => C<X>();
}

// NEGATIVE upstream test_staticFunctionTearoff_instantiatedWithTypeVariable
// (a type-variable instantiation cannot become const).
void fStaticTearoffTypeVar<T>() {
  final c = CStaticTearTypeVar.m<T>;
}

class CStaticTearTypeVar<T> {
  static C<X> m<X>() => C<X>();
}

// POSITIVE upstream test_staticFunctionTearoff_uninstantiated (1 row at
// `x`).
void fStaticTearoffUninstantiated() {
  final x = CStaticTearUninst.m;
}

class CStaticTearUninst<T> {
  static C<X> m<X>() => C<X>();
}

// NEGATIVE upstream test_test_recordLiteral_nonConst (a non-const-able
// record literal keeps `final`).
final tupleNonConst = (1, () {});

// NEGATIVE upstream test_test_recordLiteral_ok (already `const`).
const recordOk = (number: 123, name: 'Main', type: 'Street');

// POSITIVE upstream test_topLevelVariable_final_doubleLiteral (1 row at
// `x`).
final xTopDouble = 1.3;

// NEGATIVE upstream test_topLevelVariable_final_listLiteral.
final xTopList = [];

// POSITIVE upstream test_topLevelVariable_final_listLiteral_const (1 row at
// `x`).
final xTopListConst = const [];

// NEGATIVE upstream test_topLevelVariable_final_mapOrSetLiteral.
final o9Top = {};

// POSITIVE upstream test_topLevelVariable_final_mapOrSetLiteral_const (1
// row at `x`).
final xTopMapOrSetConst = const {};

// POSITIVE upstream test_topLevelVariable_final_nullLiteral (1 row at `a`).
final aTopNull = null;

// POSITIVE upstream test_topLevelVariable_final_topLevelVariable_const (1
// row at `y`).
const xTopBase = const [];
final yTopAlias = xTopBase;

// NEGATIVE upstream test_topLevelVariable_listLiteral_const.
const xTopConstList = const [];
