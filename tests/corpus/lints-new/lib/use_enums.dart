// use_enums — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/use_enums_test.dart (transcribed 2026-10-14,
// vendored SDK 3.15 main face).
// Inventory: 9 POSITIVE + 20 NEGATIVE + 1 SKIPPED = all 30 upstream
// tests. SKIPPED (1): test_augmentation — the augmentation-libraries face
// (`augment class`) needs the augmentations experiment, which this corpus
// cannot mount (the noDiagnostic control is preserved by its upstream
// TODO issue 4900 note; the simple part face below covers the hasPart
// shape). Upstream cases each compiled standalone; declarations carry
// block-suffixed names. Primary-constructor faces ride the
// primary-constructors feature at the corpus language version 3.13.
// Expected rows (oracle leg): 9 — USE_ENUMS keyed use_enums.dart, one per
// POSITIVE block at the class name.

// The hasPart face's companion (upstream test_simple_hasPart writes
// lib/a.dart as `part of 'test.dart'`).
part 'use_enums_a.dart';

// POSITIVE upstream test_constructor_private (1 row at the class name).
class UEPrivCtor {
  static const UEPrivCtor a = UEPrivCtor._(1);
  static const UEPrivCtor b = UEPrivCtor._(2);
  final int value;
  const UEPrivCtor._(this.value);
}

// POSITIVE upstream test_extendsObject (1 row at the class name; `extends
// Object` is implicit and does not save the face).
class UEExtendsObject extends Object {
  static const UEExtendsObject a = UEExtendsObject._();
  static const UEExtendsObject b = UEExtendsObject._();
  const UEExtendsObject._();
}

// POSITIVE upstream test_multiDeclaration (1 row at the class name; both
// constants ride one declaration).
class UEMultiDecl {
  static const UEMultiDecl a = UEMultiDecl._(), b = UEMultiDecl._();
  const UEMultiDecl._();
}

// NEGATIVE upstream test_no_lint_abstract (an abstract candidate is not
// convertible).
abstract class UEAbstract {
  static const UEAbstract a = UESub();
  static const UEAbstract b = UESub();
  const UEAbstract();
}

class UESub extends UEAbstract {
  const UESub();
}

// NEGATIVE upstream test_no_lint_constructor_factory.
class UEFactory {
  static const UEFactory a = UEFactory();
  static const UEFactory b = UEFactory();
  factory UEFactory.f(int index) {
    throw '!';
  }
  const UEFactory();
}

// NEGATIVE upstream test_no_lint_constructor_named.
class UENamedCtor {
  static const UENamedCtor a = UENamedCtor.x();
  static const UENamedCtor b = UENamedCtor.x();
  const UENamedCtor.x();
}

// NEGATIVE upstream test_no_lint_constructor_public.
class UEPublicCtor {
  static const UEPublicCtor a = UEPublicCtor(1);
  static const UEPublicCtor b = UEPublicCtor(2);
  final int index;
  const UEPublicCtor(this.index);
}

// NEGATIVE upstream test_no_lint_constructorUsedInConstructor (no lint;
// the recursiveConstantConstructor and unusedElementParameter diagnostics
// are outside the lint filter).
class UEUsedInCtor {
  static const UEUsedInCtor a = UEUsedInCtor();
  static const UEUsedInCtor b = UEUsedInCtor();

  const UEUsedInCtor({UEUsedInCtor e = const UEUsedInCtor()});
}

// NEGATIVE upstream test_no_lint_constructorUsedOutsideClass.
class UEUsedOutside {
  static const UEUsedOutside a = UEUsedOutside();
  static const UEUsedOutside b = UEUsedOutside();

  const UEUsedOutside();
}

UEUsedOutside ueUsedOutsideTop() => UEUsedOutside();

// NEGATIVE upstream test_no_lint_extended (a subclass invokes the private
// constructor).
class UEExtended {
  static const UEExtended a = UEExtended._();
  static const UEExtended b = UEExtended._();
  const UEExtended._();
}

class UEExtendedSub extends UEExtended {
  const UEExtendedSub() : super._();
}

// NEGATIVE upstream test_no_lint_extends_notObject.
class UEBase {
  const UEBase();
}

class UEExtendsBase extends UEBase {
  static const UEExtendsBase a = UEExtendsBase._();
  static const UEExtendsBase b = UEExtendsBase._();
  const UEExtendsBase._();
}

// NEGATIVE upstream test_no_lint_factoryAll.
class UEFactoryAll {
  static UEFactoryAll c = UEFactoryAll();
  static UEFactoryAll d = UEFactoryAll();

  factory UEFactoryAll() => c;
}

// NEGATIVE upstream test_no_lint_factorySome.
class UEFactorySome {
  static UEFactorySome c0 = UEFactorySome._();
  static UEFactorySome c1 = UEFactorySome();

  factory UEFactorySome() => c0;
  const UEFactorySome._();
}

// NEGATIVE upstream test_no_lint_implemented.
class UEImplemented {
  static const UEImplemented c = UEImplemented();
  static const UEImplemented d = UEImplemented();

  const UEImplemented();
}

class UEImplementor implements UEImplemented {}

// NEGATIVE upstream test_no_lint_implements_index_field.
class UEIndexField {
  static const UEIndexField a = UEIndexField._();
  static const UEIndexField b = UEIndexField._();
  const UEIndexField._();
  final int index = 0;
}

// NEGATIVE upstream test_no_lint_implements_index_getter.
class UEIndexGetter {
  static const UEIndexGetter a = UEIndexGetter._();
  static const UEIndexGetter b = UEIndexGetter._();
  const UEIndexGetter._();
  int get index => 0;
}

// NEGATIVE upstream test_no_lint_implements_values_field.
class UEValuesField {
  static const UEValuesField a = UEValuesField._();
  static const UEValuesField b = UEValuesField._();
  const UEValuesField._();
  static final List<UEValuesField> values = [a, b];
}

// NEGATIVE upstream test_no_lint_implements_values_method.
class UEValuesMethod {
  static const UEValuesMethod a = UEValuesMethod._();
  static const UEValuesMethod b = UEValuesMethod._();
  const UEValuesMethod._();
  static List<UEValuesMethod> values() => [];
}

// NEGATIVE upstream test_no_lint_nonConstConstructor.
class UENonConstCtor {
  static final UENonConstCtor a = UENonConstCtor();
  static final UENonConstCtor b = UENonConstCtor();

  UENonConstCtor();
}

// NEGATIVE upstream test_no_lint_nonInstanceCreationInitialization.
class UENonCreationInit {
  static const UENonCreationInit a = UENonCreationInit();
  static const UENonCreationInit b = a;

  const UENonCreationInit();
}

// NEGATIVE upstream test_no_lint_overrides_equals.
class UEOverridesEquals {
  static const UEOverridesEquals a = UEOverridesEquals();
  static const UEOverridesEquals b = UEOverridesEquals();
  const UEOverridesEquals();
  @override
  bool operator ==(Object other) => false;
}

// NEGATIVE upstream test_no_lint_overrides_hashCode.
class UEOverridesHashCode {
  static const UEOverridesHashCode a = UEOverridesHashCode();
  static const UEOverridesHashCode b = UEOverridesHashCode();
  const UEOverridesHashCode();
  @override
  int get hashCode => 0;
}

// NEGATIVE upstream test_primaryConstructor_hashCode (primary-constructors
// face).
class const UEPrimaryHash._(final int hashCode) {
  static const UEPrimaryHash a = UEPrimaryHash._(1);
  static const UEPrimaryHash b = UEPrimaryHash._(2);
}

// NEGATIVE upstream test_primaryConstructor_index (primary-constructors
// face).
class const UEPrimaryIndex._(final int index) {
  static const UEPrimaryIndex a = UEPrimaryIndex._(1);
  static const UEPrimaryIndex b = UEPrimaryIndex._(2);
}

// NEGATIVE upstream test_primaryConstructor_noLint_nonConst
// (primary-constructors face).
class UEPrimaryNonConst(int x) {
  static final UEPrimaryNonConst a = UEPrimaryNonConst(1);
  static final UEPrimaryNonConst b = UEPrimaryNonConst(2);
}

// POSITIVE upstream test_primaryConstructor_private (1 row at the class
// name; primary-constructors face).
class const UEPrimaryPriv._(final int value) {
  static const UEPrimaryPriv a = UEPrimaryPriv._(1);
  static const UEPrimaryPriv b = UEPrimaryPriv._(2);
}

// NEGATIVE upstream test_primaryConstructor_public (primary-constructors
// face).
class const UEPrimaryPub(final int value) {
  static const UEPrimaryPub a = UEPrimaryPub(1);
  static const UEPrimaryPub b = UEPrimaryPub(2);
}

// NEGATIVE upstream test_primaryConstructor_values (primary-constructors
// face).
class const UEPrimaryValues._(final int values) {
  static const UEPrimaryValues a = UEPrimaryValues._(1);
  static const UEPrimaryValues b = UEPrimaryValues._(2);
}

// POSITIVE upstream test_referencedFactoryConstructor (1 row at the class
// name; the factory is referenced outside).
class UEFactoryReferenced {
  static const UEFactoryReferenced c = UEFactoryReferenced();
  static const UEFactoryReferenced d = UEFactoryReferenced();

  const UEFactoryReferenced();

  factory UEFactoryReferenced.withValue(int x) => c;
}

UEFactoryReferenced ueFactoryRef = UEFactoryReferenced.withValue(0);

// POSITIVE upstream test_simple_hasPart (1 row at the class name; the part
// companion above).
class UEHasPart {
  static const UEHasPart a = UEHasPart._();
  static const UEHasPart b = UEHasPart._();
  const UEHasPart._();
}

// POSITIVE upstream test_simple_private (1 row at the class name).
class UEPrivSimple {
  static const UEPrivSimple a = UEPrivSimple();
  static const UEPrivSimple b = UEPrivSimple();
  const UEPrivSimple();
}

// POSITIVE upstream test_simple_public (1 row at the class name).
class UEPubSimple {
  static const UEPubSimple a = UEPubSimple._();
  static const UEPubSimple b = UEPubSimple._();
  const UEPubSimple._();
}

// POSITIVE upstream test_withMixin (1 row at the class name).
mixin class UEMixin {}

class UEWithMixin with UEMixin {
  static const UEWithMixin a = UEWithMixin._();
  static const UEWithMixin b = UEWithMixin._();
  const UEWithMixin._();
}
