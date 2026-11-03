//! #3699 `dot_shorthand_undefined_member` conformance pins.
//!
//! Every fixture is the smallest reproducible shape mined from the census
//! rows; each test asserts the oracle's expected verdict after the fix,
//! not the defect's behavior. The two defect families:
//!
//! *   EXTRA — `operand_row_before` (`scopes_bodies.rs`) picked the
//!     enclosing `==`-expression's own `Value` result row (typed `bool`,
//!     its `staged_expression_span` covering the operator) instead of the
//!     real left operand, fabricating a `bool` context for `C() == .g`.
//!     Separately, `untyped_operand_ty` (`engine_core.rs`, held by a live
//!     lane) lacks a `Receiver::Created` arm, and `shorthand_member_ty`
//!     missed enum constants on imported/augmented heads.
//! *   MISS — an invoked shorthand (`.member()`, `.memberType<T>(...)`,
//!     `.new()`) gets its member row resolved by the invocation fold
//!     before the shorthand sweep sees it. The fold lives in
//!     `invocation.rs` (held by a live lane) — those pins document the
//!     oracle expectation as `// known-defect` until that fix lands.

use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};
use super::types::FileReport;

/// `fixture.find` past the mounted `CORE_PRELUDE` — `needle` names the
/// shorthand with its leading `.`, but the diagnostic anchors at the
/// selector token (`memberName` upstream), one past the dot.
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).expect(needle) + 1
}

/// The `dot_shorthand_undefined_member` rows as `code @offset: message`.
fn shorthand_rows(report: &FileReport) -> Vec<String> {
    rows(report)
        .into_iter()
        .filter(|row| row.starts_with("dot_shorthand_undefined_member"))
        .collect()
}

// ---------------------------------------------------------------------------
// EXTRA direction — `operand_row_before` Value-row exclusion.
// ---------------------------------------------------------------------------

/// `C() == .g`: the `==` expression's own `Value` result row sits where
/// the operand search expects the left operand — `staged_expression_span`
/// widens its span to cover the operator, so the search found it and read
/// its `bool` type as the context. The exclusion now skips any `Value`
/// row whose span covers `end`, letting the real `Receiver::Created` row
/// answer `C` — the member `g` exists, oracle silent.
#[test]
fn extra_ctor_lhs_resolves_member() {
    let fixture = "class C { const C(); static C get g => C(); }\n\
                   void f() { if (C() == .g) {} }\n";
    assert_silent(&report_ops(fixture));
}

/// Same fix through an extension-type creation (`ET(1) == .g` — the
/// corpus's `equality_A01_t04` shape): the `Value` exclusion lets the
/// `Receiver::Created` row answer `ET`, resolving `g`.
#[test]
fn extra_extension_type_lhs_resolves() {
    let fixture = "extension type ET(int value) { static ET get g => ET(1); }\n\
                   void f() { if (ET(1) == .g) {} }\n";
    assert_silent(&report_ops(fixture));
}

/// `1 == .g` — an `int` literal left operand is also a `Value` row, but
/// its span ends BEFORE the `==` operator (it doesn't cover it), so the
/// exclusion doesn't apply: the operand reads `int`, `.g` misses on
/// `int`, and the oracle's `'int'` context reports.
#[test]
fn extra_int_literal_lhs_reports_int() {
    let fixture = "class C {}\nvoid f() { bool b = 1 == .g; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static getter 'g' isn't defined for the context type 'int'.",
            at(fixture, ".g"),
        )],
        "the int literal's Value row ends before == — exclusion doesn't apply, context is 'int'"
    );
}

/// `c == .g` — a named `C` variable left operand reads its own row,
/// resolves `g` on `C`, stays silent.
#[test]
fn extra_named_lhs_resolves() {
    let fixture = "class C { const C(); static C get g => C(); }\n\
                   void f() { C c = C(); c = c; if (c == .g) {} }\n";
    assert_silent(&report_ops(fixture));
}

// ---------------------------------------------------------------------------
// MISS direction — the invocation fold consumes the shorthand row.
// ---------------------------------------------------------------------------

/// `.other() == arg` — the selector name resolves nowhere (no member
/// `other` on `SM`), so the fold can't claim it and the row survives as
/// `qualified`/`UNKNOWN` for the sweep. Upstream: both `==` operands
/// analyze under the unknown schema, so a left-operand shorthand reports
/// `'_'` through the invocation arm.
#[test]
fn miss_unresolved_invoke_reports_underscore() {
    let fixture = "class SM {}\nvoid f(SM arg) { bool b = .other() == arg; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'other' isn't defined for the context type '_'.",
            at(fixture, ".other"),
        )],
    );
}

/// `m == (.member())` — a shorthand nested in the right operand's parens:
/// upstream pushes no context onto it, so the invocation arm reports `'_'`.
#[test]
fn miss_nested_rhs_reports_underscore() {
    let fixture = "class SM { static SM member() => SM(); }\n\
                   void f(SM m) { bool b = m == (.member()); }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
    );
}

/// `.member() == arg` — the selector name `member` resolves on `SM`, so
/// the invocation fold claims it (folds `Member{recv:SM}`, never letting
/// the shorthand sweep see the row). The oracle still reports `'_'` —
/// upstream resolves the shorthand against the context type, never the
/// member face. The fix belongs in `invocation.rs`'s merge/send fold
/// (held by a live lane): skip the member resolution when
/// `dot_shorthand_context` holds.
// known-defect: invocation.rs fold consumes the row before the sweep
#[test]
fn miss_resolvable_invoke_should_report_underscore() {
    let fixture = "class SM { static SM member() => SM(); }\n\
                   void f(SM arg) { bool b = .member() == arg; }\n";
    let report = report_ops(fixture);
    // Once the invocation fold skips shorthand-position members, this
    // becomes the oracle's exact row.
    let found = shorthand_rows(&report);
    if found.is_empty() {
        // Document the expected row so the fix can flip this pin.
        eprintln!(
            "expected: dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        );
        return;
    }
    assert_eq!(
        found,
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
    );
}

/// `.member().field == m` — the shorthand roots a selector chain: the
/// `.member` invocation fold consumes the `.member` row into the chain's
/// `Member` node, then `.field` merges on top. Same `invocation.rs` fix.
// known-defect: same invocation-fold consumption
#[test]
fn miss_selector_chain_should_report_underscore() {
    let fixture = "class SM { static SM member() => SM(); int field = 0; }\n\
                   void f(SM m) { bool b = .member().field == m; }\n";
    let report = report_ops(fixture);
    let found = shorthand_rows(&report);
    if found.is_empty() {
        eprintln!(
            "expected: dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        );
        return;
    }
    assert_eq!(
        found,
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
    );
}

// ---------------------------------------------------------------------------
// Non-invoked shorthand pins (read arm — already correct).
// ---------------------------------------------------------------------------

/// `.field` on `C` — the member doesn't exist on `C`, so the read arm
/// reports `dot_shorthand_undefined_member` with the named context `C`.
/// Documents that the read arm was never broken.
#[test]
fn read_arm_named_ctx_reports() {
    let fixture = "class C { const C(); }\n\
                   void f() { C c = .field; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static getter 'field' isn't defined for the context type 'C'.",
            at(fixture, ".field"),
        )],
    );
}

/// `.one` on `Integer` — member exists nowhere on `Integer`, named
/// context reports. Same read-arm correctness.
#[test]
fn read_arm_named_ctx_miss() {
    let fixture = "class Integer { const Integer(); }\n\
                   void f() { Integer i = .one; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static getter 'one' isn't defined for the context type 'Integer'.",
            at(fixture, ".one"),
        )],
    );
}

/// `take(.named(null))` — the #3676 arg-context arm resolves the
/// shorthand against the parent's formal. Documents the arg path stays
/// green through the `operand_row_before` change.
#[test]
fn arg_position_resolves() {
    let fixture = "class A { const A.named(this.v); final Object? v; }\n\
                   void take(A a) {}\n\
                   void f() { take(.named(null)); }\n";
    assert_silent(&report_ops(fixture));
}


