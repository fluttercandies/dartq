// unnecessary_this_alias — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/unnecessary_this_alias_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 18 POSITIVE + 7 NEGATIVE = all 25 upstream tests; SKIPPED
// (1): test_noDiagnostic_experimentDisabled — it demands the
// anonymous_methods experiment be DISABLED, which this corpus cannot
// express (the root analysis_options enables the experiment corpus-wide
// for the two anonymous-method faces; an options-level experiment applies
// to every library, so a disabled-experiment control is unmountable — the
// unnecessary_ignore DisabledTest skip family).
// The two anonymous-method faces (`self.{ ... }`, `self?.{ ... }`) parse
// only with the anonymous_methods experiment, enabled in the root
// analysis_options (`analyzer.enable-experiment`) — the same face upstream
// mounts via experimentalFeatures.
// Expected rows (oracle leg): 18 — UNNECESSARY_THIS_ALIAS keyed
// unnecessary_this_alias.dart, one per POSITIVE block at the `self`/`x`
// variable name. The primary-constructor face (`class C(var int x, ...)`)
// rides the primary-constructors feature at the corpus language version.

// NEGATIVE upstream test_noDiagnostic_differentType (the alias targets a
// different type; no this-promotion applies).
class TADiff {
  void m() {
    Object self = this;
    if (self is TADiffSub) {}
  }
}

class TADiffSub extends TADiff {}

// NEGATIVE upstream test_noDiagnostic_late (a `late var` alias is not
// promotable).
class TALate {
  void m() {
    late var self = this;
    if (self is TALateSub) {}
  }
}

class TALateSub extends TALate {}

// NEGATIVE upstream test_noDiagnostic_mutated (the alias is reassigned).
class TAMutated {
  void m() {
    var self = this;
    if (self is TAMutated) {}
    self = TAMutated();
  }
}

// NEGATIVE upstream test_noDiagnostic_noPromotionUsage (the alias is never
// used in a promotion).
class TANoPromotion {
  void m() {
    var self = this;
    print(self);
  }
}

// NEGATIVE upstream test_noDiagnostic_notThis (the initializer is a
// parameter, not `this`).
class TANotThis {
  void m(TANotThis other) {
    var self = other;
    if (self is TANotThisSub) {}
  }
}

class TANotThisSub extends TANotThis {}

// NEGATIVE upstream test_thisPromotion_anonymousMethod_notNullAware (a
// non-null-aware anonymous-method invocation does not demand promotion).
extension TAExtNotNullAware on TANullAwareBase? {
  void m() {
    var self = this;
    self.{
      m();
    };
  }
}

class TANullAwareBase {
  void m() {}
}

// POSITIVE upstream test_thisPromotion_anonymousMethod_nullAware (1 row at
// `self`; the null-aware anonymous-method face).
extension TAExtNullAware on TANullAwareBase? {
  void m() {
    var self = this;
    self?.{
      m();
    };
  }
}

// POSITIVE upstream test_thisPromotion_as (1 row at `self`).
class TAAs {
  void m() {
    var self = this;
    (self as TAAsSub).foo();
  }
}

class TAAsSub extends TAAs {
  void foo() {}
}

// POSITIVE upstream test_thisPromotion_bang (1 row at `self`).
extension TABang on TABangBase? {
  void m() {
    var self = this;
    self!;
  }
}

class TABangBase {}

// POSITIVE upstream test_thisPromotion_bangEqNull (1 row at `self`).
extension TABangEqNull on TABangEqNullBase? {
  void m() {
    var self = this;
    if (self != null) {}
  }
}

class TABangEqNullBase {}

// POSITIVE upstream test_thisPromotion_eqEqNull (1 row at `self`).
extension TAEqEqNull on TAEqEqNullBase? {
  void m() {
    var self = this;
    if (self == null) {}
  }
}

class TAEqEqNullBase {}

// POSITIVE upstream test_thisPromotion_ifCase (1 row at `self`).
class TAIfCase {
  void m() {
    var self = this;
    if (self case TAIfCaseSub _) {}
  }
}

class TAIfCaseSub extends TAIfCase {}

// POSITIVE upstream test_thisPromotion_is (1 row at `self`).
class TAIs {
  void m() {
    var self = this;
    if (self is TAIsSub) {}
  }
}

class TAIsSub extends TAIs {}

// POSITIVE upstream test_thisPromotion_isNot (1 row at `self`).
class TAIsNot {
  void m() {
    var self = this;
    if (self is! TAIsNotSub) {}
  }
}

class TAIsNotSub extends TAIsNot {}

// POSITIVE upstream test_thisPromotion_nullAwareCascade (1 row at `self`).
extension TANullCascade on TANullCascadeBase? {
  void m() {
    var self = this;
    self?..m();
  }
}

class TANullCascadeBase {
  void m() {}
}

// POSITIVE upstream test_thisPromotion_nullAwareIndexAccess (1 row at
// `self`).
extension TANullIndex on TANullIndexBase? {
  void m() {
    var self = this;
    self?[0];
  }
}

class TANullIndexBase {
  operator [](int index) {}
}

// POSITIVE upstream test_thisPromotion_nullAwareMethodInvocation (1 row at
// `self`).
extension TANullMethod on TANullMethodBase? {
  void m() {
    var self = this;
    self?.m();
  }
}

class TANullMethodBase {
  void m() {}
}

// POSITIVE upstream test_thisPromotion_nullAwarePropertyAccess (1 row at
// `self`; primary-constructors face).
extension TANullProperty on TANullPropertyBase? {
  void m() {
    var self = this;
    self?.x = self.y;
  }
}

class TANullPropertyBase(var int x, var int y);

// POSITIVE upstream test_thisPromotion_nullBangEq (1 row at `self`).
extension TANullBangEq on TANullBangEqBase? {
  void m() {
    var self = this;
    if (null != self) {}
  }
}

class TANullBangEqBase {}

// POSITIVE upstream test_thisPromotion_nullEqEq (1 row at `self`).
extension TANullEqEq on TANullEqEqBase? {
  void m() {
    var self = this;
    if (null == self) {}
  }
}

class TANullEqEqBase {}

// POSITIVE upstream test_thisPromotion_parenthesized (1 row at `self`; the
// parenthesized initializer does not defeat the alias face).
class TAParenthesized {
  void m() {
    var self = (this);
    if (self is TAParenthesizedSub) {}
  }
}

class TAParenthesizedSub extends TAParenthesized {}

// POSITIVE upstream test_thisPromotion_parenthesizedUsage (1 row at
// `self`; the parenthesized usage still reports).
extension TAParenUsage on TAParenUsageBase? {
  void m() {
    var self = this;
    if ((self) == null) {}
  }
}

class TAParenUsageBase {}

// POSITIVE upstream test_thisPromotion_switch (1 row at `self`).
class TASwitch {
  void m() {
    var self = this;
    switch (self) {
      case TASwitchSub _:
    }
  }
}

class TASwitchSub extends TASwitch {}

// POSITIVE upstream test_thisPromotion_switchExpr (1 row at `x`).
class TASwitchExpr {
  void m(TASwitchExpr self) {
    var x = this;
    var y = switch (x) {
      TASwitchExprSub _ => 1,
      _ => 2,
    };
  }
}

class TASwitchExprSub extends TASwitchExpr {}
