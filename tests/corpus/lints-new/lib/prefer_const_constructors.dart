// prefer_const_constructors — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/prefer_const_constructors_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 17 POSITIVE + 29 NEGATIVE = all 46 upstream tests, none
// skipped. The @literal faces resolve through the corpus's vendored
// package:meta. Dot-shorthand and primary-constructor faces ride features
// enabled at the corpus language version 3.13. The error-only cases
// (canBeConst_literal, canBeConst_literal_dotShorthands) are NEGATIVE for
// the lint face (nonConstCallToLiteralConstructor error is outside the
// filter). Upstream cases compiled standalone; declarations carry
// block-suffixed names; the two deferred-import cases share one helper
// file (deferred_helper.dart — see that file's header).
// Expected rows (oracle leg): 18 — PREFER_CONST_CONSTRUCTORS keyed
// prefer_const_constructors.dart (site counts in block headers).

// Whole-file imports (upstream declares these per case; hoisted because a
// merged corpus file keeps its directives at the top).
import 'package:meta/meta.dart';
import 'deferred_helper.dart' deferred as a;

// POSITIVE upstream test_canBeConst_argumentIsAdjacentStrings (1 row).
class AAdjacent {
  const AAdjacent(String s);
}
var aAdjacent = AAdjacent('adjacent' 'string');

// POSITIVE upstream test_canBeConst_argumentIsListLiteral (1 row).
class AListArg {
  const AListArg(List<int> l);
}
var aListArg = AListArg([]);

// NEGATIVE upstream test_canBeConst_argumentIsMap_nonLiteral.
class AMapNonLiteral {
  const AMapNonLiteral(Map<int, int> m);
}
AMapNonLiteral fMapNonLiteral(Map<int, int> m) => AMapNonLiteral(m);

// NEGATIVE upstream test_canBeConst_argumentIsMapLiteral_containsNonLiteral.
class AMapContainsNonLiteral {
  const AMapContainsNonLiteral(Map<int, int> m);
}
AMapContainsNonLiteral fMapContainsNonLiteral(int x) =>
    AMapContainsNonLiteral({x: x});

// POSITIVE upstream test_canBeConst_argumentIsMapLiteral_instantiated
// (1 row).
class AMapInstantiated {
  const AMapInstantiated(Map<int, int> m);
}
var aMapInstantiated = AMapInstantiated({});

// POSITIVE upstream test_canBeConst_dotShorthand (1 row at `.new()`).
class ADotNew {
  const ADotNew();
}
ADotNew aDotNew = .new();

// POSITIVE upstream test_canBeConst_dotShorthand_methodCall (1 row at
// `.tightFor(height: 400)`).
class ADotMethod {
  const ADotMethod.tightFor({int? height});
  ADotMethod copyWith({int? minWidth}) => this;
}
void fDotMethod() {
  ADotMethod a = .tightFor(height: 400).copyWith(minWidth: 400);
}

// POSITIVE upstream test_canBeConst_explicitTypeArgument_dynamic (1 row).
class ADynArg<T> {
  const ADynArg();
}
var aDynArg = ADynArg<dynamic>();

// POSITIVE upstream test_canBeConst_explicitTypeArgument_string (1 row).
class AStringArg<T> {
  const AStringArg();
}
var aStringArg = AStringArg<String>();

// NEGATIVE upstream test_canBeConst_implicitTypeArgument.
class AImplicit<T, U> {
  const AImplicit();
}
AImplicit<T, int> fImplicit<T>() => AImplicit();

// POSITIVE upstream test_canBeConst_implicitTypeArgument_downwardInference
// (1 row).
class ADownward<T> {
  const ADownward();
}
ADownward<int> fDownward() => ADownward();

// NEGATIVE upstream test_canBeConst_implicitTypeArgument_inConditional.
class AConditional<T, U> {
  const AConditional();
}
class BConditional<T, U> extends AConditional<T, U> {}
AConditional<T, int> fConditional<T>(bool b) => b ? AConditional() : BConditional();

// POSITIVE upstream test_canBeConst_intLiteralArgument (1 row).
class AIntArg {
  const AIntArg(int x);
}
var aIntArg = AIntArg(5);

// NEGATIVE upstream test_canBeConst_literal (no lint; the
// nonConstCallToLiteralConstructor error is outside the lint filter).
class KLiteral {
  @literal
  const KLiteral();
}

KLiteral kLiteral = KLiteral();

// NEGATIVE upstream test_canBeConst_literal_dotShorthands (no lint; same
// error face through a dot shorthand).
class KLiteralDot {
  @literal
  const KLiteralDot();
}

KLiteralDot kLiteralDot = .new();

// POSITIVE upstream test_canBeConst_newSyntax (1 row; the `const new()`
// declaring-constructor spelling rides the primary-constructors feature).
class ANewSyntax {
  const new();
}
ANewSyntax aNewSyntax = ANewSyntax();

// POSITIVE upstream test_canBeConst_optionalNamedParameter (1 row).
class AOptionalNamed {
  const AOptionalNamed({AOptionalNamed? parent});
}
var aOptionalNamed = AOptionalNamed();

// POSITIVE upstream test_canBeConst_optionalNamedParameter_nested (2 rows:
// the outer and inner invocations).
class ANested {
  const ANested({ANested? parent});
  const ANested.a();
}
var aNested = ANested(
  parent: ANested.a(),
);

// POSITIVE upstream test_canBeConst_optionalNamedParameter_newKeyword (1
// row at `new A()`).
class ANewKeyword {
  const ANewKeyword({ANewKeyword? parent});
}
var aNewKeyword = new ANewKeyword();

// NEGATIVE upstream
// test_cannotBeConst_argumentIsAdjacentStrings_withInterpolation.
class AInterp {
  const AInterp(String s);
}
AInterp fInterp(int i) => AInterp('adjacent' '$i');

// NEGATIVE upstream
// test_cannotBeConst_argumentIsExtensionTypeMethodCall.
final class AExtTypeArg {
  final Ex f;
  const AExtTypeArg(this.f);

  void foo() {
    var a = AExtTypeArg(-f);
  }
}

extension type const Ex(int i) implements int {
  Ex operator -() => Ex(-i);
}

// NEGATIVE upstream
// test_cannotBeConst_argumentIsListLiteral_nonLiteralElement.
class AListNonLiteral {
  const AListNonLiteral(List<int> l);
}
AListNonLiteral fListNonLiteral(int i) => AListNonLiteral([i]);

// NEGATIVE upstream test_cannotBeConst_argumentIsLocalVariable.
class ALocalVar {
  const ALocalVar(String s);
}

void fLocalVar() {
  final s = '';
  var a = ALocalVar(s);
}

// NEGATIVE upstream test_cannotBeConst_argumentIsNonLiteral.
class ANonLiteral {
  const ANonLiteral(String s);
}
ANonLiteral fNonLiteral(String s) => ANonLiteral(s);

// NEGATIVE upstream test_cannotBeConst_argumentIsNonLiteralList.
class ANonLiteralList {
  const ANonLiteralList(List<int> l);
}
ANonLiteralList fNonLiteralList(List<int> l) => ANonLiteralList(l);

// NEGATIVE upstream test_cannotBeConst_dotShorthands_issue60963.
class ANotConst {
  int cannotBeConst;
  ANotConst(): cannotBeConst = 0;
}
extension type const BDotShorthand(ANotConst a) {}

BDotShorthand get bDotShorthand => .new(ANotConst());

// NEGATIVE upstream
// test_cannotBeConst_explicitTypeArgument_typeVariable_constructorDeclaration.
class ATypeVarCtor<T> {
  const ATypeVarCtor({required List<T> p});

  factory ATypeVarCtor.named() => ATypeVarCtor(p: <T>[]);
}

// NEGATIVE upstream
// test_cannotBeConst_explicitTypeArgument_typeVariable_functionDeclaration.
class ATypeVarFn<T> {
  const ATypeVarFn();
}
void fTypeVarFn<U>() => ATypeVarFn<U>();

// NEGATIVE upstream test_cannotBeConst_nonConstArgument.
class ANonConstArg {
  final int x;

  const ANonConstArg(this.x);
}
ANonConstArg fNonConstArg(int x) => ANonConstArg(x);

// NEGATIVE upstream test_cannotBeConst_notConstConstructor.
class ANotConstCtor {
  ANotConstCtor();
}
var aNotConstCtor = ANotConstCtor();

// NEGATIVE upstream test_cannotBeConst_notConstConstructor_dotShorthand.
class ANotConstDot {
  ANotConstDot();
}
ANotConstDot aNotConstDot = .new();

// NEGATIVE upstream
// test_cannotBeConst_stringLiteralArgument_withInterpolation.
class AStringInterp {
  const AStringInterp(String s);
  static AStringInterp m1(int i) => AStringInterp('$i');
}

// POSITIVE upstream test_class_primaryConstructor_const (1 row;
// primary-constructors face).
class const CPrimaryConst(final int x);
var cPrimaryConst = CPrimaryConst(1);

// POSITIVE upstream test_class_primaryConstructor_dotShorthand_const (1
// row; primary-constructors face).
class const CPrimaryDot(final int x);
CPrimaryDot get fPrimaryDot => .new(1);

// POSITIVE upstream test_class_primaryConstructor_named_const (1 row;
// primary-constructors face).
class const CPrimaryNamed.named(final int x);
var cPrimaryNamed = CPrimaryNamed.named(1);

// NEGATIVE upstream test_constructorArgument_rhsOfLogicalOperation.
class CLogicalRhs {
  final bool x;
  const CLogicalRhs(this.x);
}
const CLogicalRhs aLogicalRhs = CLogicalRhs(true);
final CLogicalRhs bLogicalRhs = CLogicalRhs(false || aLogicalRhs.x);

// NEGATIVE upstream test_deferred_arg (helper: deferred_helper.dart, class
// A + const aa).
class BDeferred {
  const BDeferred(Object a);
}

main() {
  var b = BDeferred(a.aa);
}

// NEGATIVE upstream test_deferredConstructorCall (same helper).
void fDeferredCtorCall() {
  var aa = a.A();
}

// NEGATIVE upstream test_dotShorthand_withConst.
class ADotWithConst {
  const ADotWithConst.tightFor({int? height});
}
void fDotWithConst() {
  ADotWithConst a = const .tightFor(height: 400);
}

// NEGATIVE upstream test_dotShorthand_withConst_methodCall.
class ADotWithConstMethod {
  const ADotWithConstMethod.tightFor({int? height});
  ADotWithConstMethod copyWith({int? minWidth}) => this;
}
void fDotWithConstMethod() {
  ADotWithConstMethod a =
      const .tightFor(height: 400).copyWith(minWidth: 400);
}

// POSITIVE upstream test_extensionType_constPrimaryConstructor (1 row).
extension type const EConstPrimary(int i) {}

var eConstPrimary = EConstPrimary(1);

// NEGATIVE upstream test_extensionType_nonConstPrimaryConstructor.
extension type ENonConstPrimary(int i) {}

var eNonConstPrimary = ENonConstPrimary(1);

// NEGATIVE upstream test_isConst_intLiteralArgument.
class AIsConst {
  final int x;

  const AIsConst(this.x);
}
AIsConst fIsConst() => const AIsConst(5);

// NEGATIVE upstream test_isConstCall_optionalNamedParameter.
class AIsConstCall {
  const AIsConstCall({AIsConstCall? parent});
}
var aIsConstCall = const AIsConstCall();

// NEGATIVE upstream test_objectConstructorCall.
var xObjectCtor = Object();

// NEGATIVE upstream test_objectConstructorCall_dotShorthand.
Object xObjectDot = .new();
