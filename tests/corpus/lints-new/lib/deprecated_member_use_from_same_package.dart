// deprecated_member_use_from_same_package — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/deprecated_member_use_from_same_package_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 47 POSITIVE + 24 NEGATIVE in-file cases = 71 of the 77
// upstream tests; the 6 directive cases (deprecatedClass_usedIn{Hide,Show}
// Combinator + deprecatedLibrary_{export,_export_inPart,_export_inSubpart,
// _import}) need sibling helper libraries and live under
// deprecated_member_use_from_same_package/ (five of the six directive
// subfiles carry rows; the hide face is silent). Expected rows (oracle
// leg): 52 lint sites — 47 in this file plus 5 in the subfiles (block
// headers carry per-block counts). Upstream compiled each case standalone; declarations carry
// block-suffixed names (the rule keys on resolved deprecations, so renames
// are face-neutral). Markers normalized to plain source.

// POSITIVE upstream test_deprecatedCallMethod (1 row at `c()`).
class CCall {
  @deprecated
  void call() {}
}

void fCall(CCall c) => c();

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedClassTypeAlias.
@deprecated
mixin class CDepAlias {}

@deprecated
class DFromDepAlias = Object with CDepAlias;

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedDefaultParameter.
@deprecated
class CDepDefault {}

void fDepDefault({@deprecated CDepDefault? c = null}) {}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedEnum.
@deprecated
class CDepEnum {}

@deprecated
enum EDepEnum {
  one, two;

  void f(CDepEnum c) {}
}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedExtension.
@deprecated
class CDepExt {}

@deprecated
extension EDepExt on CDepExt {}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedExtensionTypeRepresentation.
@deprecated
class CDepExtType {}

@deprecated
extension type EDepExtType(CDepExtType c) { }

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedField_initializer.
@deprecated
class CDepFieldInit {}

class DFieldInit {
  @deprecated
  Object f = CDepFieldInit;
}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedField_typeAnnotation.
@deprecated
class CDepFieldType {}

class DFieldType {
  @deprecated
  CDepFieldType? f;
}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedFieldFormalParameter.
@deprecated
class CDepFieldFormal {}

class DFieldFormal {
  Object c;
  DFieldFormal({@deprecated required CDepFieldFormal this.c});
}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedFunctionTypeAlias.
@deprecated
class CDepFnAlias {}

@deprecated
typedef void CallbackDep(CDepFnAlias c);

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedFunctionTypedParameter.
@deprecated
class CDepFnTyped {}

void fFnTyped({@deprecated required void p(CDepFnTyped c)}) {}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedLibrary.
@deprecated
library aDepLib;

@deprecated
class CDepLib {}

CDepLib? xDepLib;

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedMixin.
@deprecated
class CDepMixin {}

@deprecated
mixin MDepMixin {
  CDepMixin? x;
}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedSimpleParameter.
@deprecated
class CDepSimple {}

void fDepSimple({@deprecated CDepSimple? c}) {}

// NEGATIVE upstream test_deprecatedClass_usedInDeprecatedTopLevelVariable.
@deprecated
class CDepTopVar {}

@deprecated
CDepTopVar? xDepTopVar;

// NEGATIVE upstream test_deprecatedClass_usedInGenericFunctionTypeAlias.
@deprecated
class CDepGenericAlias {}

@deprecated
typedef CallbackGeneric = void Function(CDepGenericAlias);

// NEGATIVE upstream test_deprecatedMethod_usedInDeprecatedConstructor.
class CDepCtorBody {
  @deprecated
  void m() {}

  @deprecated
  CDepCtorBody() {
    m();
  }
}

// NEGATIVE upstream
// test_deprecatedMethod_usedInDeprecatedSubclassConstructor.
class CDepSubBase {
  @deprecated
  void m() {}
}
class CDepSub extends CDepSubBase {
  @deprecated
  CDepSub() {
    m();
  }
}

// NEGATIVE upstream
// test_deprecatedNamedParameterOfFunction_usedInDeclaringFunction.
int? fNamedInDeclaring({@deprecated int? p}) => p;

// NEGATIVE upstream
// test_deprecatedParameterOfConstructor_usedInDeclaringConstructorBody.
class CParamDeclaringBody {
  CParamDeclaringBody({@deprecated int? p}) {
    p;
  }
}

// NEGATIVE upstream
// test_deprecatedParameterOfConstructor_usedInDeclaringConstructorInitializer.
class CParamDeclaringInit {
  CParamDeclaringInit({@deprecated int? p}) : assert(p == null || p > 0);
}

// NEGATIVE upstream
// test_deprecatedParameterOfPrimaryConstructor_usedInDeclaringConstructorBody.
class CParamPrimaryDeclaring(@deprecated int? p) {
  this : q = p;
  int? q;
}

// POSITIVE upstream test_deprecatedClass (1 row at `C`).
@deprecated
class CDepUse {}

void fDepUse(CDepUse c) {}

// POSITIVE upstream test_deprecatedClass_usedInClassTypeAlias (1 row at
// `C`).
@deprecated
mixin class CAlias {}

class DAlias = Object with CAlias;

// POSITIVE upstream test_deprecatedClass_usedInExtensionTypeRepresentation
// (1 row at `C`).
@deprecated
class CExtRepr {}

extension type EExtRepr(CExtRepr c) { }

// POSITIVE upstream test_deprecatedClass_usedInFieldFormalParameter (1 row
// at `C`).
@deprecated
class CFieldFormal {}

class DFieldFormalUse {
  Object c;
  DFieldFormalUse({required CFieldFormal this.c});
}

// POSITIVE upstream test_deprecatedClass_usedInFunctionTypeAlias (1 row at
// `C`).
@deprecated
class CFnAlias {}

typedef void CallbackUse(CFnAlias c);

// POSITIVE upstream test_deprecatedClass_usedInFunctionTypedParameter
// (1 row at `C`).
@deprecated
class CFnTyped {}

void fFnTypedUse({void p(CFnTyped c)?}) {}

// POSITIVE upstream test_deprecatedClass_usedInGenericTypeAlias (1 row at
// `C`).
@deprecated
class CGenericAlias {}

typedef CallbackGenericUse = void Function(CGenericAlias);

// POSITIVE upstream
// test_deprecatedConstructor_usedInRedirectingConstructorInitializer (1 row
// at `this()`).
class CRedirecting {
  @deprecated
  CRedirecting();
  CRedirecting.two() : this();
}

// POSITIVE upstream test_deprecatedConstructor_usedInSuperConstructorCall
// (1 row at `super()`).
class CSuperBase {
  @deprecated
  CSuperBase();
}
class CSuperUse extends CSuperBase {
  CSuperUse() : super();
}

// POSITIVE upstream
// test_deprecatedConstructor_usedInSuperConstructorCall_implicit (1 row at
// the implicit `B();`).
class CImplicitBase {
  @deprecated
  CImplicitBase();
}
class CImplicitUse extends CImplicitBase {
  CImplicitUse();
}

// POSITIVE upstream test_deprecatedDefaultParameterOfFunction (1 row at
// `p`).
void fDefaultParam({@deprecated int p = 1}) {}

void gDefaultParam() => fDefaultParam(p: 1);

// POSITIVE upstream test_deprecatedEnum (1 row at `E`).
@deprecated
enum EDepUse {
  one, two;
}
late EDepUse eDepUse;

// POSITIVE upstream test_deprecatedEnumValue (1 row at `two`).
enum EDepValue {
  one, @deprecated two;
}
late EDepValue eDepValue = EDepValue.two;

// POSITIVE upstream test_deprecatedExtension_usedInExtensionOverride_getter
// (1 row at `E`).
@deprecated
extension EExtOverrideGetter on int {
  int get foo => 1;
}

var xExtOverrideGetter = EExtOverrideGetter(0).foo;

// POSITIVE upstream
// test_deprecatedExtension_usedInExtensionOverride_methodInvocation (1 row
// at `E`).
@deprecated
extension EExtOverrideMethod on int {
  void f() {}
}

var xExtOverrideMethod = EExtOverrideMethod(0).f();

// POSITIVE upstream
// test_deprecatedExtensionType_usedInExtensionTypeImplements (1 row at
// `E`).
@deprecated
extension type EExtImpl(int i) { }

extension type FExtImpl(int i) implements EExtImpl { }

// POSITIVE upstream
// test_deprecatedExtensionType_usedInExtensionTypeRepresentation (1 row at
// `E`).
@deprecated
extension type EExtReprUse(int i) { }

extension type FExtReprUse(EExtReprUse c) { }

// POSITIVE upstream test_deprecatedExtensionType_usedInField (1 row at
// `E`).
@deprecated
extension type EExtField(int i) { }

class CExtField {
  EExtField? e;
}

// POSITIVE upstream test_deprecatedExtensionType_usedInFunctionParam (1 row
// at `E`).
@deprecated
extension type EExtParam(int i) { }

void fExtParam(EExtParam e) { }

// POSITIVE upstream test_deprecatedField_inObjectPattern_explicitName (1
// row at `foo`).
class CPatternField {
  @Deprecated('')
  final int foo = 0;
}

int fPatternField(Object x) =>
  switch (x) {
    CPatternField(foo: var bar) => bar,
    _ => 0,
  };

// POSITIVE upstream test_deprecatedField_inObjectPattern_inferredName (1
// row at `foo`).
class CPatternInferred {
  @Deprecated('')
  final int foo = 0;
}

int fPatternInferred(Object x) =>
  switch (x) {
    CPatternInferred(:var foo) => foo,
    _ => 0,
  };

// POSITIVE upstream test_deprecatedField_usedAsGetter (1 row at `a.f`).
class CFieldGetter {
  @deprecated
  int f = 0;
}

void fFieldGetter(CFieldGetter a) {
  a.f;
}

// POSITIVE upstream test_deprecatedField_usedAsSetter (1 row at `a.f`).
class CFieldSetter {
  @deprecated
  int f = 0;
}

void fFieldSetter(CFieldSetter a) {
  a.f = 1;
}

// NEGATIVE upstream test_deprecatedField_usedInDeprecatedClass.
class CFieldInDepBase {
  @deprecated
  int f = 0;
}

@deprecated
class BFieldInDep {
  void f(CFieldInDepBase a) {
    a.f;
    a.f = 1;
  }
}

// NEGATIVE upstream test_deprecatedField_usedInDeprecatedFunction.
class CFieldInDepFnBase {
  @deprecated
  int f = 0;
}

@deprecated
void fFieldInDepFn(CFieldInDepFnBase a) {
  a.f;
  a.f = 1;
}

// POSITIVE upstream test_deprecatedField_usedInPostfix (1 row at `a.f`).
class CFieldPostfix {
  @deprecated
  int f = 0;
}

void fFieldPostfix(CFieldPostfix a) {
  a.f++;
}

// POSITIVE upstream test_deprecatedField_usedInPrefix (1 row at `a.f`).
class CFieldPrefix {
  @deprecated
  int f = 0;
}

void fFieldPrefix(CFieldPrefix a) {
  ++a.f;
}

// POSITIVE upstream test_deprecatedField_usedInSuper (1 row at `super.f`).
class CFieldSuperBase {
  @deprecated
  int f = 0;
}

class CFieldSuper extends CFieldSuperBase {
  int get g => super.f;
}

// NEGATIVE upstream test_deprecatedGetter_usedInAssignment: the assignment
// targets the non-deprecated setter, so no row.
@deprecated
int get xGetterAssign => 1;

set xGetterAssign(int value) {}

void fGetterAssign() {
  xGetterAssign = 1;
}

// POSITIVE upstream test_deprecatedGetter_usedInCompoundAssignment (1 row
// at `x`).
@deprecated
int get xGetter => 1;

set xGetter(int value) {}

void fGetterCompound() {
  xGetter += 1;
}

// POSITIVE upstream test_deprecatedIndexOperator (1 row at `c[1]`).
class CIndexOp {
  @deprecated
  int operator[](int p) => 1;
}

void fIndexOp(CIndexOp c) {
  c[1];
}

// POSITIVE upstream test_deprecatedMethod (1 row at `m`).
class CMethod {
  @deprecated
  void m() {}

  void m2() {
    m();
  }
}

// POSITIVE upstream test_deprecatedMethod_usedInPrimaryConstructorBody
// (1 row at `deprecatedMethod`).
@deprecated
void deprecatedPrimaryMethod() {}

class CPrimaryBody(var int x) {
  this {
    deprecatedPrimaryMethod();
  }
}

// POSITIVE upstream test_deprecatedMethod_withMessage (1 row at `m`).
class CMethodMessage {
  @Deprecated('Message')
  void m() {}

  void m2() {
    m();
  }
}

// POSITIVE upstream test_deprecatedNamedConstructor (1 row at `C.named`).
class CNamedCtor {
  @deprecated
  CNamedCtor.named();
}

var xNamedCtor = CNamedCtor.named();

// POSITIVE upstream
// test_deprecatedNamedConstructor_usedInSuperConstructorCall (1 row at
// `super.named()`).
class CSuperNamedBase {
  @deprecated
  CSuperNamedBase.named();
}
class CSuperNamed extends CSuperNamedBase {
  CSuperNamed() : super.named();
}

// POSITIVE upstream test_deprecatedNamedParameterOfFunction (1 row at
// `p`).
void fNamedParam({@deprecated int? p}) {}

void gNamedParam() => fNamedParam(p: 1);

// POSITIVE upstream test_deprecatedNamedParameterOfMethod (1 row at `p`).
class CNamedParamMethod {
  void f({@deprecated int? p}) {}

  void g() => f(p: 1);
}

// POSITIVE upstream test_deprecatedOperator (1 row at `c + 1`).
class COperator {
  @deprecated
  COperator operator+(int other) => COperator();
}
void fOperator(COperator c) {
  c + 1;
}

// POSITIVE upstream test_deprecatedOperator_usedInCompoundAssignment (1 row
// at `c += 1`).
class COperatorCompound {
  @deprecated
  COperatorCompound operator+(int other) => COperatorCompound();
}
void fOperatorCompound(COperatorCompound c) {
  c += 1;
}

// POSITIVE upstream
// test_deprecatedParameterOfConstructor_usedInRedirectingConstructor (1 row
// at `p`).
class CParamRedirecting {
  CParamRedirecting({@deprecated int? p});
  CParamRedirecting.two() : this(p: 0);
}

// POSITIVE upstream test_deprecatedPositionalParameterOfFunction (1 row at
// the argument `1`).
void fPositionalParam([@deprecated int? p]) {}

void gPositionalParam() => fPositionalParam(1);

// POSITIVE upstream test_deprecatedPrimaryConstructor_bodyPart (1 row at
// `A`).
class CPrimaryDeprecated(var int x) {
  @deprecated
  this;
}

void fPrimaryDeprecated() {
  var a = CPrimaryDeprecated(1);
}

// POSITIVE upstream test_deprecatedSetter (1 row at `f`).
@deprecated set fTopSetter(int value) {}

void gTopSetter() => fTopSetter = 1;

// POSITIVE upstream test_deprecatedSetter_usedInCompoundAssignment (1 row
// at `x`).
int get xSetterBase => 1;

@deprecated
set xSetterBase(int value) {}

void fSetterCompound() {
  xSetterBase += 1;
}

// POSITIVE upstream test_deprecatedStaticField (1 row at `A.f`).
class CStaticField {
  @deprecated
  static int f = 0;
}

var aStaticField = CStaticField.f;

// POSITIVE upstream test_deprecatedTopLevelVariable_usedInAssignment (1 row
// at `x`).
@deprecated
int xTopVar = 1;

void fTopVarAssign() {
  xTopVar = 1;
}

// POSITIVE upstream test_deprecatedUnnamedConstructor (1 row at `C`).
class CUnnamedCtor {
  @deprecated
  CUnnamedCtor();
}

var xUnnamedCtor = CUnnamedCtor();

// POSITIVE upstream test_deprecatedUnnamedConstructor_newSyntax (1 row at
// `C`; the `new();` declaring-constructor spelling rides the
// primary-constructors feature, enabled at the corpus language version).
class CNewSyntax {
  @deprecated
  new();
}

var xNewSyntax = CNewSyntax();
