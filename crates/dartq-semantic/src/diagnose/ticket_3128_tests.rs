//! The #3128 pins: `operator==` member-entry capture — `formals` and
//! `return_ty` in the collector model.
//!
//! A `==` operator declaration captures like any other user-definable
//! operator: the name token is the `==` spelling itself (no
//! `MethodDeclaration` identifier event ever follows it), the return
//! annotation parses before `operator` and re-parses through the
//! return-type re-walk, and `(Object o)` / `(B o)` feeds the executable's
//! own depth-0 formal list. The captured `MemberEntry` must read
//! `kind=Method`, `formals=Some([param])`, `return_ty=<written annotation>`
//! — never `formals=None` (which would force `override_broken`'s
//! `(None, Some)` suppress arm) and never `return_ty=<container>` (the
//! `extends` clause's armed type leaking through the operator latch).
//!
//! These are COLLECTOR pins (the `FileElements` model): the diagnoser's
//! `invalid_override` verdict is `#3127`'s separate face and only becomes
//! reachable once the model carries the honest signature.
//!
//! Oracle provenance: `dart analyze` 3.13.7 — `class B extends A { bool
//! operator==(B o) => true; }` over `bool operator==(Object o)` emits
//! `invalid_override` at `==(B`, message `'B.=='`.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The `invalid_override` rows as `(code, offset, len)` sorted by offset.
fn overrides(report: &crate::diagnose::FileReport) -> Vec<(&'static str, u32, u32)> {
    let mut v: Vec<(&'static str, u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    v.sort_by_key(|(code, offset, _)| (*offset, *code));
    v
}

/// `bool operator==(B)` over `bool operator==(Object)` — the contravariant
/// parameter face: `B` does not accept `Object`, so `invalid_override` at
/// the `==` name token, display `'B.=='`. The `=` in `==` is the operator's
/// own spelling, never a setter marker.
#[test]
fn eq_param_mismatch_reports_with_eq_eq_display() {
    let fixture = "class A{bool operator==(Object o)=>true;}\nclass B extends A{bool operator==(B o)=>true;}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "==(B"), 2)],
        "rows: {:?}",
        rows(&report)
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_override")
        .unwrap()
        .message;
    assert!(
        msg.contains("'B.=='"),
        "operator == must render intact, not stripped to '=': {msg}"
    );
}

/// `int operator==(Object)` over `bool operator==(Object)` — the covariant
/// return face: `int` does not fit `bool`, so `invalid_override`.
#[test]
fn eq_return_mismatch_reports() {
    let fixture = "class A{bool operator==(Object o)=>true;}\nclass B extends A{int operator==(Object o)=>0;}\n";
    let report = report_ops(fixture);
    // B's `==` token — the needle anchors at B's `int` return and `+12`
    // walks `int operator` to the `==` token.
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "int operator==") + 12, 2)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `bool operator==(B)` on a class whose super does NOT declare `==` —
/// `Object` still answers `==` through the implicit `Object` face, so the
/// contravariant parameter rule applies and `invalid_override` reports.
/// (A class body `==` always overrides `Object.==`.)
#[test]
fn eq_on_plain_class_still_overrides_object() {
    let fixture = "class A{}\nclass B{bool operator==(B o)=>true;}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "==(B"), 2)],
        "rows: {:?}",
        rows(&report)
    );
}

/// `bool operator==(Object)` over `bool operator==(Object)` — the honest
/// override (the `Object` parameter accepts the super's `Object`), silent.
#[test]
fn eq_honest_override_is_silent() {
    let fixture = "class A{bool operator==(Object o)=>true;}\nclass B extends A{bool operator==(Object o)=>true;}\n";
    let report = report_ops(fixture);
    assert!(overrides(&report).is_empty(), "rows: {:?}", rows(&report));
}

/// Controls: `+`/`[]=` param mismatches report exactly like `==`'s — the
/// same contravariant operator-parameter face.
#[test]
fn plus_and_index_set_param_controls() {
    let fixture =
        "class A{num operator+(num o)=>0;}\nclass B extends A{int operator+(int o)=>0;}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "+(int"), 1)],
        "rows: {:?}",
        rows(&report)
    );
    let fixture = "class A{void operator[]=(int i,num v){}}\nclass B extends A{void operator[]=(int i,String v){}}\n";
    let report = report_ops(fixture);
    assert_eq!(
        overrides(&report),
        vec![("invalid_override", at(fixture, "[]=(int i,String"), 3)],
        "rows: {:?}",
        rows(&report)
    );
}
