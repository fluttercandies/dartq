// matching_super_parameters — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/matching_super_parameters_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 12 POSITIVE + 24 NEGATIVE = all 36 upstream tests, none
// skipped. Primary-constructor faces ride the primary-constructors feature
// (enabled at the corpus language version 3.13). Upstream's /*[n*/.../*n]*/
// markers are normalized away; marked sites are named per block. The
// nonMatching_tooMany cases expect a superFormalParameterWithoutAssociated
// Positional ERROR (outside the lint filter) and NO lint.
// Expected rows (oracle leg): 20 — MATCHING_SUPER_PARAMETERS keyed
// matching_super_parameters.dart (site counts in the block headers).

// NEGATIVE upstream test_inBodyToInBody_explicitInvocation_matchingWithOffset.
class D1 extends C1 {
  final String w;

  D1(
    this.w,
    super.x,
    super.y,
  ) : super.named();
}

class C1 {
  final int x;
  final int y;

  C1.named(this.x, this.y);
}

// NEGATIVE upstream
// test_inBodyToInBody_explicitInvocation_matchingWithOffset_withNamedExplicit.
class D2 extends C2 {
  final String w;

  D2(
    this.w,
    super.x,
    super.y,
  ) : super.named(z: 1);
}

class C2 {
  final int x;
  final int y;
  final int z;

  C2.named(this.x, this.y, {required this.z});
}

// NEGATIVE upstream
// test_inBodyToInBody_explicitInvocation_matchingWithOffset_withNamedSuper.
class D3 extends C3 {
  final String w;

  D3(
    this.w,
    super.x,
    super.y, {
    required super.z,
  }) : super.named();
}

class C3 {
  final int x;
  final int y;
  final int z;

  C3.named(this.x, this.y, {required this.z});
}

// POSITIVE upstream test_inBodyToInBody_explicitInvocation_nonMatching
// (2 rows: `super.y` then `super.x`).
class D4 extends C4 {
  final String w;

  D4(
    this.w,
    super.y,
    super.x,
  ) : super.named();
}

class C4 {
  final int x;
  final int y;

  C4.named(this.x, this.y);
}

// NEGATIVE upstream
// test_inBodyToInBody_implicitInvocation_matchingWithGap.
class D5 extends C5 {
  final String w;

  D5(
    super.x,
    this.w,
    super.y,
  );
}

class C5 {
  final int x;
  final int y;

  C5(this.x, this.y);
}

// NEGATIVE upstream
// test_inBodyToInBody_implicitInvocation_matchingWithOffset.
class D6 extends C6 {
  final String w;

  D6(
    this.w,
    super.x,
    super.y,
  );
}

class C6 {
  final int x;
  final int y;

  C6(this.x, this.y);
}

// POSITIVE upstream test_inBodyToInBody_implicitInvocation_nonMatching
// (2 rows: `super.y` then `super.x`).
class D7 extends C7 {
  final String w;

  D7(
    this.w,
    super.y,
    super.x,
  );
}

class C7 {
  final int x;
  final int y;

  C7(this.x, this.y);
}

// POSITIVE upstream
// test_inBodyToInBody_implicitInvocation_nonMatching_omittedOptional (1 row
// at `super.y`).
class D8 extends C8 {
  final String w;

  D8(
    this.w,
    super.y,
  );
}

class C8 {
  final int x;
  final int? y;

  C8(this.x, [this.y]);
}

// NEGATIVE upstream
// test_inBodyToInBody_implicitInvocation_nonMatching_tooMany (no lint; the
// super-formal-without-associated-parameter error kills matching).
class D9 extends C9 {
  final String w;

  D9(
    this.w,
    super.y,
    super.x,
  );
}

class C9 {
  final int x;

  C9(this.x);
}

// NEGATIVE upstream
// test_inBodyToPrimary_explicitInvocation_matchingWithOffset
// (primary-constructors face).
class D11 extends C11 {
  final String w;

  D11(this.w, super.x, super.y) : super.named();
}

class C11.named(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_inBodyToPrimary_explicitInvocation_matchingWithOffset_withNamedExplicit
// (primary-constructors face).
class D12 extends C12 {
  final String w;

  D12(this.w, super.x, super.y) : super.named(z: 1);
}

class C12.named(this.x, this.y, {required this.z}) {
  final int x;
  final int y;
  final int z;
}

// NEGATIVE upstream
// test_inBodyToPrimary_explicitInvocation_matchingWithOffset_withNamedSuper
// (primary-constructors face).
class D13 extends C13 {
  final String w;

  D13(this.w, super.x, super.y, {required super.z}) : super.named();
}

class C13.named(this.x, this.y, {required this.z}) {
  final int x;
  final int y;
  final int z;
}

// POSITIVE upstream test_inBodyToPrimary_explicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D14 extends C14 {
  final String w;

  D14(this.w, super.y, super.x) : super.named();
}

class C14.named(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_inBodyToPrimary_implicitInvocation_matchingWithGap
// (primary-constructors face).
class D15 extends C15 {
  final String w;

  D15(super.x, this.w, super.y);
}

class C15(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_inBodyToPrimary_implicitInvocation_matchingWithOffset
// (primary-constructors face).
class D16 extends C16 {
  final String w;

  D16(this.w, super.x, super.y);
}

class C16(this.x, this.y) {
  final int x;
  final int y;
}

// POSITIVE upstream test_inBodyToPrimary_implicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D17 extends C17 {
  final String w;

  D17(this.w, super.y, super.x);
}

class C17(this.x, this.y) {
  final int x;
  final int y;
}

// POSITIVE upstream
// test_inBodyToPrimary_implicitInvocation_nonMatching_omittedOptional (1
// row at `super.y`; primary-constructors face).
class D18 extends C18 {
  final String w;

  D18(this.w, super.y);
}

class C18(this.x, [this.y]) {
  final int x;
  final int? y;
}

// NEGATIVE upstream
// test_inBodyToPrimary_implicitInvocation_nonMatching_tooMany (no lint;
// primary-constructors face).
class D19 extends C19 {
  final String w;

  D19(this.w, super.y, super.x);
}

class C19(this.x) {
  final int x;
}

// NEGATIVE upstream
// test_primaryToInBody_explicitInvocation_matchingWithOffset
// (primary-constructors face).
class D20(this.w, super.x, super.y) extends C20 {
  final String w;

  this : super.named();
}

class C20 {
  final int x;
  final int y;

  C20.named(this.x, this.y);
}

// NEGATIVE upstream
// test_primaryToInBody_explicitInvocation_matchingWithOffset_withNamedExplicit
// (primary-constructors face).
class D21(this.w, super.x, super.y) extends C21 {
  final String w;

  this : super.named(z: 1);
}

class C21 {
  final int x;
  final int y;
  final int z;

  C21.named(this.x, this.y, {required this.z});
}

// NEGATIVE upstream
// test_primaryToInBody_explicitInvocation_matchingWithOffset_withNamedSuper
// (primary-constructors face).
class D22(this.w, super.x, super.y, {required super.z}) extends C22 {
  final String w;

  this : super.named();
}

class C22 {
  final int x;
  final int y;
  final int z;

  C22.named(this.x, this.y, {required this.z});
}

// POSITIVE upstream test_primaryToInBody_explicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D23(this.w, super.y, super.x) extends C23 {
  final String w;

  this : super.named();
}

class C23 {
  final int x;
  final int y;

  C23.named(this.x, this.y);
}

// NEGATIVE upstream
// test_primaryToInBody_implicitInvocation_matchingWithGap
// (primary-constructors face).
class D24(super.x, this.w, super.y) extends C24 {
  final String w;
}

class C24 {
  final int x;
  final int y;

  C24(this.x, this.y);
}

// NEGATIVE upstream
// test_primaryToInBody_implicitInvocation_matchingWithOffset
// (primary-constructors face).
class D25(this.w, super.x, super.y) extends C25 {
  final String w;
}

class C25 {
  final int x;
  final int y;

  C25(this.x, this.y);
}

// POSITIVE upstream test_primaryToInBody_implicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D26(this.w, super.y, super.x) extends C26 {
  final String w;
}

class C26 {
  final int x;
  final int y;

  C26(this.x, this.y);
}

// POSITIVE upstream
// test_primaryToInBody_implicitInvocation_nonMatching_omittedOptional (1
// row at `super.y`; primary-constructors face).
class D27(this.w, super.y) extends C27 {
  final String w;
}

class C27 {
  final int x;
  final int? y;

  C27(this.x, [this.y]);
}

// NEGATIVE upstream
// test_primaryToInBody_implicitInvocation_nonMatching_tooMany (no lint;
// primary-constructors face).
class D28(this.w, super.y, super.x) extends C28 {
  final String w;
}

class C28 {
  final int x;

  C28(this.x);
}

// NEGATIVE upstream
// test_primaryToPrimary_explicitInvocation_matchingWithOffset
// (primary-constructors face).
class D29(this.w, super.x, super.y) extends C29 {
  final String w;

  this : super.named();
}

class C29.named(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_primaryToPrimary_explicitInvocation_matchingWithOffset_withNamedExplicit
// (primary-constructors face).
class D30(this.w, super.x, super.y) extends C30 {
  final String w;

  this : super.named(z: 1);
}

class C30.named(this.x, this.y, {required this.z}) {
  final int x;
  final int y;
  final int z;
}

// NEGATIVE upstream
// test_primaryToPrimary_explicitInvocation_matchingWithOffset_withNamedSuper
// (primary-constructors face).
class D31(this.w, super.x, super.y, {required super.z}) extends C31 {
  final String w;

  this : super.named();
}

class C31.named(this.x, this.y, {required this.z}) {
  final int x;
  final int y;
  final int z;
}

// POSITIVE upstream test_primaryToPrimary_explicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D32(this.w, super.y, super.x) extends C32 {
  final String w;

  this : super.named();
}

class C32.named(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_primaryToPrimary_implicitInvocation_matchingWithGap
// (primary-constructors face).
class D33(super.x, this.w, super.y) extends C33 {
  final String w;
}

class C33(this.x, this.y) {
  final int x;
  final int y;
}

// NEGATIVE upstream
// test_primaryToPrimary_implicitInvocation_matchingWithOffset
// (primary-constructors face).
class D34(this.w, super.x, super.y) extends C34 {
  final String w;
}

class C34(this.x, this.y) {
  final int x;
  final int y;
}

// POSITIVE upstream test_primaryToPrimary_implicitInvocation_nonMatching
// (2 rows; primary-constructors face).
class D35(this.w, super.y, super.x) extends C35 {
  final String w;
}

class C35(this.x, this.y) {
  final int x;
  final int y;
}

// POSITIVE upstream
// test_primaryToPrimary_implicitInvocation_nonMatching_omittedOptional (1
// row at `super.y`; primary-constructors face).
class D36(this.w, super.y) extends C36 {
  final String w;
}

class C36(this.x, [this.y]) {
  final int x;
  final int? y;
}

// NEGATIVE upstream
// test_primaryToPrimary_implicitInvocation_nonMatching_tooMany (no lint;
// primary-constructors face).
class D37(this.w, super.y, super.x) extends C37 {
  final String w;
}

class C37(this.x) {
  final int x;
}
