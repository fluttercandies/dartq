// avoid_redundant_argument_values — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/avoid_redundant_argument_values_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 21 POSITIVE + 14 NEGATIVE = 35 of the 36 upstream tests.
// SKIPPED (1): test_fromEnvironment — upstream marks it @FailingTest
// (upstream issue 3447); transcribing it would score the oracle's
// known-internal failure as a differential verdict. Primary-constructor
// and dot-shorthand faces ride features enabled at the corpus language
// version 3.13. Upstream cases compiled standalone; declarations carry
// block-suffixed names.
// Expected rows (oracle leg): 21 — AVOID_REDUNDANT_ARGUMENT_VALUES keyed
// avoid_redundant_argument_values.dart (site counts in block headers).

// NEGATIVE upstream test_annotation: an annotation invocation never
// reports.
@ARedundantAnnotation(p: false)
class ARedundantAnnotation {
  final bool p;
  const ARedundantAnnotation({this.p = true});
}

// POSITIVE upstream test_annotation_redundant (1 row at `true`).
@ARedundantAnnUse(p: true)
class ARedundantAnnUse {
  final bool p;
  const ARedundantAnnUse({this.p = true});
}

// POSITIVE upstream test_constructor_inBody_redundant (1 row at `true`).
void fCtorBody() {
  ARedundantBody(p: true);
}
class ARedundantBody {
  ARedundantBody({bool p = true});
}

// POSITIVE upstream test_constructor_inBody_tearoff_redundant (1 row at
// `true`).
void fCtorTearoff() {
  var aNew = ARedundantTearoff.new;
  aNew(p: true);
}
class ARedundantTearoff {
  ARedundantTearoff({bool p = true});
}

// POSITIVE upstream test_constructor_primary_redundant (1 row at `true`;
// primary-constructors face).
void fCtorPrimary() {
  ARedundantPrimary(p: true);
}
class ARedundantPrimary({bool p = true});

// POSITIVE upstream test_dotShorthand (1 row at `true`).
class CDotShorthand {
  static CDotShorthand other({foo = true}) => CDotShorthand();
}
CDotShorthand cDotShorthand = .other(foo: true);

// POSITIVE upstream test_dotShorthandConstructor (1 row at `true`).
class CDotShorthandCtor({foo = true});
CDotShorthandCtor cDotShorthandCtor = .new(foo: true);

// POSITIVE upstream test_enumDeclaration (1 row at `false`; upstream issue
// 3617).
enum TestEnumRedundant {
  a(test: false);

  const TestEnumRedundant({this.test = false});

  final bool test;
}

// NEGATIVE upstream
// test_function_optionalPositional_followedByPositional.
void fFollowedByPositional() {
  gFollowedByPositional(0, 1);
}
void gFollowedByPositional([int a = 0, int? b]) {}

// NEGATIVE upstream
// test_function_optionalPositional_subsequent_different.
void fSubsequentDifferent() {
  gSubsequentDifferent(0, 2);
}
void gSubsequentDifferent([int? a, int? b = 1]) {}

// POSITIVE upstream
// test_function_optionalPositional_subsequent_redundant (1 row at `1`).
void fSubsequentRedundant() {
  gSubsequentRedundant(0, 1);
}
void gSubsequentRedundant([int? a, int? b = 1]) {}

// NEGATIVE upstream test_functionType_optionalNamed_nullable_null.
void fFnTypeNamedNull(void Function({int? p}) g) {
  g(p: null);
}

// NEGATIVE upstream test_functionType_optionalPositional_nullable_null.
void fFnTypePositionalNull(void Function([int? p]) g) {
  g(null);
}

// NEGATIVE upstream test_localFunction_optionalNamed_different.
void fLocalFnDifferent() {
  void gLocalDiff({bool p = true}) {}
  gLocalDiff(p: false);
}

// POSITIVE upstream test_localFunction_optionalNamed_redundant (1 row at
// `true`).
void fLocalFnRedundant() {
  void gLocalRed({bool p = true}) {}
  gLocalRed(p: true);
}

// NEGATIVE upstream test_method_generic_nonNullDefault_null.
void fMethodGenericNull(AGenericNull<String> a) {
  a.gGenericNull('x', null);
}
class AGenericNull<T> {
  void gGenericNull(T a, [int? p = 1]) {}
}

// POSITIVE upstream
// test_method_generic_optionalPositional_nullable_null (1 row at `null`).
void fMethodGenericNullable(A<int> a) {
  a.gGenericNullable(null);
}
class AGenericNullable<T> {
  void gGenericNullable([T? p]) {}
}

// NEGATIVE upstream test_method_noDefault.
void fMethodNoDefault(ANoDefault a) {
  a.gNoDefault(p: false);
}
class ANoDefault {
  void gNoDefault({bool? p}) {}
}

// NEGATIVE upstream test_method_optionalNamed_variable.
void fMethodNamedVar(ANamedVar a, bool v) {
  a.gNamedVar(p: v);
}
class ANamedVar {
  void gNamedVar({bool p = true}) {}
}

// POSITIVE upstream test_method_redundant (1 row at `true`).
void fMethodRedundant(ARedundantMethod a) {
  a.gRedundant(p: true);
}
class ARedundantMethod {
  void gRedundant({bool p = true}) {}
}

// POSITIVE upstream test_named_nullable_null (1 row at `null`).
void gNamedNullable({int? p}) {}

void fNamedNullable() {
  gNamedNullable(p: null);
}

// POSITIVE upstream test_named_nullable_null_default (1 row at `null`).
void gNamedNullableDefault({int? p = null}) {}

void fNamedNullableDefault() {
  gNamedNullableDefault(p: null);
}

// POSITIVE upstream test_namedArgumentBeforePositional (1 row at `true`).
void fooNamedBefore(int a, int b, {bool c = true}) {}

void fNamedBefore() {
  fooNamedBefore(0, c: true, 1);
}

// POSITIVE upstream test_optional_positional_nullable_null (1 row at
// `null`).
void gOptionalNullable([int? p]) {}

void fOptionalNullable() {
  gOptionalNullable(null);
}

// NEGATIVE upstream test_redirectingFactoryConstructor.
class AFactoryRedirect {
  factory AFactoryRedirect([int? value]) = BFactoryRedirect;
  AFactoryRedirect._();
}
class BFactoryRedirect extends AFactoryRedirect {
  BFactoryRedirect([int? value = 2]) : super._();
}
void fFactoryRedirect() {
  AFactoryRedirect();
  AFactoryRedirect(null);
  AFactoryRedirect(1);
}

// NEGATIVE upstream test_redirectingFactoryConstructor_cyclic (no lint;
// the recursiveFactoryRedirect errors are outside the lint filter).
class ACyclicFactory {
  factory ACyclicFactory.foo() = ACyclicFactory.bar;
  factory ACyclicFactory.bar() = ACyclicFactory.foo;
}
void fCyclicFactory() {
  ACyclicFactory.foo();
}

// NEGATIVE upstream test_redirectingFactoryConstructor_multipleOptional.
class AMultipleOptional {
  factory AMultipleOptional([int? one, int? two]) = BMultipleOptional;
  AMultipleOptional._();
}
class BMultipleOptional extends AMultipleOptional {
  int? one;
  int? two;
  BMultipleOptional([this.one = 2, this.two = 2]) : super._();
}
void fMultipleOptional() {
  AMultipleOptional();
  AMultipleOptional(null, null);
  AMultipleOptional(1, 1);
}

// NEGATIVE upstream test_redirectingFactoryConstructor_named.
class ANamedFactory {
  factory ANamedFactory({int? value}) = BNamedFactory;
  ANamedFactory._();
}
class BNamedFactory extends ANamedFactory {
  BNamedFactory({int? value = 2}) : super._();
}
void fNamedFactory() {
  ANamedFactory();
  ANamedFactory(value: null);
  ANamedFactory(value: 1);
}

// POSITIVE upstream test_redirectingFactoryConstructor_named_redundant
// (1 row at `2`).
class ANamedFactoryRed {
  factory ANamedFactoryRed({int? value}) = BNamedFactoryRed;
  ANamedFactoryRed._();
}
class BNamedFactoryRed extends ANamedFactoryRed {
  BNamedFactoryRed({int? value = 2}) : super._();
}
void fNamedFactoryRed() {
  ANamedFactoryRed(value: 2);
}

// NEGATIVE upstream
// test_redirectingFactoryConstructor_namedArgumentsAnywhere.
class AArgsAnywhere {
  factory AArgsAnywhere(int? one, int? two, {int? three}) = BArgsAnywhere;
  AArgsAnywhere._();
}
class BArgsAnywhere extends AArgsAnywhere {
  BArgsAnywhere(int? one, int? two, {int? three = 3}) : super._();
}
void fArgsAnywhere() {
  AArgsAnywhere(1, 2);
  AArgsAnywhere(1, three: null, 2);
  AArgsAnywhere(1, 2, three: null);
  AArgsAnywhere(1, three: 4, 2);
  AArgsAnywhere(three: 4, 1, 2);
}

// POSITIVE upstream
// test_redirectingFactoryConstructor_namedArgumentsAnywhere_redundant
// (1 row at `3`).
class AArgsAnywhereRed {
  factory AArgsAnywhereRed(int? one, int? two, {int? three}) =
      BArgsAnywhereRed;
  AArgsAnywhereRed._();
}
class BArgsAnywhereRed extends AArgsAnywhereRed {
  BArgsAnywhereRed(int? one, int? two, {int? three = 3}) : super._();
}
void fArgsAnywhereRed() {
  AArgsAnywhereRed(1, three: 3, 2);
}

// NEGATIVE upstream test_redirectingFactoryConstructor_nested.
class ANestedFactory {
  factory ANestedFactory([num? value]) = BNestedFactory;
  ANestedFactory._();
}
class BNestedFactory extends ANestedFactory {
  factory BNestedFactory([num? value]) = CNestedFactory;
  BNestedFactory._() : super._();
}
class CNestedFactory extends BNestedFactory {
  num? value;
  CNestedFactory([this.value = 2]) : super._();

  @override
  String toString() => '$value';
}
void fNestedFactory() {
  ANestedFactory();
  ANestedFactory(null);
  ANestedFactory(1);
}

// POSITIVE upstream test_redirectingFactoryConstructor_redundant (1 row at
// `2`).
class AFactoryRedundant {
  factory AFactoryRedundant([int? value]) = BFactoryRedundant;
  AFactoryRedundant._();
}
class BFactoryRedundant extends AFactoryRedundant {
  BFactoryRedundant([int? value = 2]) : super._();
}
void fFactoryRedundant() {
  AFactoryRedundant(2);
}

// NEGATIVE upstream test_redirectingGenerativeConstructor.
class AGenRedirect {
  AGenRedirect([int? value]) : this._(value);
  AGenRedirect._([int? value = 2]);
}
void fGenRedirect() {
  AGenRedirect(2);
}

// NEGATIVE upstream test_redirectingGenerativeConstructor_named.
class AGenRedirectNamed {
  AGenRedirectNamed({int? value}) : this._(value: value);
  AGenRedirectNamed._({int? value = 2});
}
void fGenRedirectNamed() {
  AGenRedirectNamed(value: 2);
}

// POSITIVE upstream
// test_redirectingGenerativeConstructor_named_redundant (1 row at `null`).
class AGenRedirectNamedRed {
  AGenRedirectNamedRed({int? value}) : this._(value: value);
  AGenRedirectNamedRed._({int? value = 2});
}
void fGenRedirectNamedRed() {
  AGenRedirectNamedRed(value: null);
}

// POSITIVE upstream test_redirectingGenerativeConstructor_redundant (1 row
// at `null`).
class AGenRedirectRed {
  AGenRedirectRed([int? value]) : this._(value);
  AGenRedirectRed._([int? value = 2]);
}
void fGenRedirectRed() {
  AGenRedirectRed(null);
}

// NEGATIVE upstream test_requiredNullable.
void fRequiredNullable({required int? x}) { }

void mainRequiredNullable() {
  fRequiredNullable(x: null);
}

// POSITIVE upstream test_toListOptionalGrowable (1 row at `true`).
void mainToList() {
  [].toList(growable: true);
}
