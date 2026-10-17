// use_named_constants — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/use_named_constants_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 8 POSITIVE + 2 NEGATIVE = all 10 upstream tests, none
// skipped. Dot-shorthand and extension-type faces ride features enabled at
// the corpus language version 3.13. Upstream cases compiled standalone;
// declarations carry block-suffixed names (the rule matches a
// reconstruction against the class's static consts by resolution, so
// renames are face-neutral).
// Expected rows (oracle leg): 8 — USE_NAMED_CONSTANTS keyed
// use_named_constants.dart, one per POSITIVE block.

// POSITIVE upstream test_constantPattern_ifCase (1 row at `A(0)`;
// upstream issue 4201).
class APatternIf {
  const APatternIf(this.value);
  final int value;

  static const zero = APatternIf(0);
}

void fPatternIf(APatternIf a) {
  if (a case const APatternIf(0)) {}
}

// POSITIVE upstream test_constantPattern_switch (1 row at `A(1)`;
// upstream issue 4201).
class APatternSwitch {
  const APatternSwitch(this.value);
  final int value;

  static const zero = APatternSwitch(0);
  static const one = APatternSwitch(1);
}

void fPatternSwitch(APatternSwitch a) {
  switch (a) {
    case const APatternSwitch(1):
  }
}

// POSITIVE upstream test_dotShorthand (1 row at `.new(0)`).
void fDotShorthand() {
  const ADot a = .new(0);
}
class ADot {
  const ADot(int value);
  static const zero = ADot(0);
}

// POSITIVE upstream test_dotShorthand_const (1 row at `const .new(0)`).
void fDotShorthandConst() {
  ADotConst a = const .new(0);
}
class ADotConst {
  const ADotConst(int value);
  static const zero = ADotConst(0);
}

// POSITIVE upstream test_extensionType (1 row at `A(0)`).
void fExtensionType() {
  const aExtType = AExtType(0);
}
extension type const AExtType(int i) {
  static const zero = AExtType(0);
}

// POSITIVE upstream test_reconstructed_sameAsPrivateName (1 row at
// `const A(1)`; the named constant is private).
void fPrivateName() {
  const APrivate(1);
}
class APrivate {
  const APrivate(int value);
  // ignore: unused_field
  static const _zero = APrivate(0);
}

// POSITIVE upstream test_reconstructed_sameAsPublicName_explicitConst
// (1 row at `const A(0)`).
void fPublicExplicit() {
  const APublicExplicit(0);
}
class APublicExplicit {
  const APublicExplicit(int value);
  static const zero = APublicExplicit(0);
}

// POSITIVE upstream test_reconstructed_sameAsPublicName_implicitConst
// (1 row at `A(0)`).
void fPublicImplicit() {
  const aPublicImplicit = APublicImplicit(0);
}
class APublicImplicit {
  const APublicImplicit(int value);
  static const zero = APublicImplicit(0);
}

// NEGATIVE upstream test_duplicate_inDefinition: the constant's own
// definition is not a reconstruction.
class ADupDef {
  const ADupDef(int value);
  static const zero = ADupDef(0);
  static const zeroAgain = ADupDef(0);
}

// NEGATIVE upstream test_usesNamed.
void fUsesNamed() {
  ANamed.zero;
}
class ANamed {
  const ANamed(int value);
  static const zero = ANamed(0);
}
