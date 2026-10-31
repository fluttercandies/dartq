//! The #3138 pins: `unused_element`'s residual faces after the
//! element-level partition — the enum-constructor self-reference leak,
//! the setter-twin read credit, the implicit-this setter write rescue,
//! and the member-only `member_refs` union.
//!
//! Upstream citations (unused_local_elements_verifier.dart, upstream Dart
//! SDK source, pkg/analyzer/lib/src/error/):
//! * The enum self-reference leak — `visitClassDeclaration` alone sets
//!   `_enclosingClass` (:94); the enum visitor never does, so an enum
//!   constructor's own `returnType` NamedType (`const _E(this._f)`'s
//!   `_E`) visits `visitNamedType` → `_useIdentifierElement` with
//!   `_enclosingClass == null`, and the `identical(element,
//!   _enclosingClass)` skip fails open (:243-246, :396-422): the enum
//!   element joins `usedElements.elements`. An enum that declares any
//!   constructor (named or unnamed) reads used; a constructor-less enum
//!   reports even when a constant writes arguments (the synthetic
//!   constructor declares no NamedType).
//! * The setter-twin read — `C()._p` on a class declaring ONLY `set _p`
//!   resolves no getter (`undefined_getter` reports), yet the setter
//!   element joins `usedElements.members` (the induced property's setter
//!   member binds the read's element slot): the setter is spared.
//! * The implicit-this setter write — a bare `_s = 1` inside the class
//!   resolves the `name=` member (preferSetter, lexical_lookup.dart:
//!   22-35); the write credits the setter element.
//! * `member_refs` invoke/write coverage — `_isUsedMember`
//!   (:977-992) checks `members` ∪ `elements`; an invoked-only member
//!   name (`C()._m()` never entering the field-read index) still
//!   resolves the member element.
//!
//! Oracle rows captured 2026-10-31 against `dart analyze --format
//! machine` 3.13.7.

use super::test_rig::report_ops;
use super::*;

/// `unused_element` rows with message text — the getter/setter faces
/// assert on the accessor's bare display name (`_p`, never `_p=`).
fn ue_rows(report: &FileReport) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

fn all_codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `(offset, length)` of `needle`'s `nth` (0-based) occurrence, shifted
/// past the prelude. Fixtures are ASCII so byte and UTF-16 offsets match.
fn at(source: &str, needle: &str, nth: usize) -> (u32, u32) {
    let found = source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing in {source:?}"));
    (
        found.0 as u32 + super::test_rig::CORE_PRELUDE.len() as u32,
        needle.len() as u32,
    )
}

// —— The enum-constructor self-reference leak ————————————————————————————

/// `enum _E { a(0); const _E(int i); }` — the declared constructor's own
/// `_E` NamedType leaks the enum element into `usedElements.elements`
/// (the upstream `_enclosingClass` gap); `_E` never reports. The `a`
/// constant's own `unused_field` verdict is the Field arm's, not this
/// face's.
#[test]
fn enum_with_declared_unnamed_ctor_is_used() {
    let report = report_ops(
        "enum _E { a(0); const _E(int i); }\n\
         void main() {}",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_E")),
        "_E reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The named-selector half: `const _E.n()`'s returnType is the same `_E`
/// NamedType — the enum reads used identically.
#[test]
fn enum_with_declared_named_ctor_is_used() {
    let report = report_ops(
        "enum _E { a.n(0); const _E.n(int i); }\n\
         void main() {}",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_E")),
        "_E reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// An empty-but-present argument list is still a constructor invocation:
/// `a()` over `const _E()` spares `_E` through the same NamedType leak —
/// the constant's own `unused_field` verdict is unchanged.
#[test]
fn enum_empty_args_with_declared_ctor_is_used() {
    let report = report_ops(
        "enum _E { a(); const _E(); }\n\
         void main() {}",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_E")),
        "_E reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The negative half: `a(0)` over a constructor-LESS enum resolves the
/// synthetic `const _E()`, which declares no `NamedType` — `_E` reports
/// at its name token exactly like the bare-constants shape.
#[test]
fn enum_without_ctor_reports_even_with_constant_args() {
    let source = "enum _E { a(0) }\n\
                  void main() {}";
    let report = report_ops(source);
    let (offset, len) = at(source, "_E", 0);
    let rows = ue_rows(&report);
    assert!(
        rows.iter()
            .any(|(o, text)| *o == offset && text.contains("_E")),
        "_E missing at {offset}+{len}: {rows:?} in {source:?}"
    );
}

/// Bare constants never reach a `NamedType` — `enum _E { a, b }` reports
/// `_E`.
#[test]
fn enum_bare_constants_still_report() {
    let source = "enum _E { a, b }\n\
                  void main() {}";
    let report = report_ops(source);
    let (offset, _) = at(source, "_E", 0);
    let rows = ue_rows(&report);
    assert!(
        rows.iter()
            .any(|(o, text)| *o == offset && text.contains("_E")),
        "_E missing at {offset}: {rows:?} in {source:?}"
    );
}

/// A private class's own constructor does NOT leak — `visitClass
/// Declaration` sets `_enclosingClass`, so `const _C()`'s `_C` NamedType
/// self-skips: the class reports exactly as before.
#[test]
fn class_with_ctor_still_reports() {
    let source = "class _C { const _C(); }\n\
                  void main() {}";
    let report = report_ops(source);
    let rows = ue_rows(&report);
    assert!(
        rows.iter().any(|(_, text)| text.contains("_C")),
        "_C missing: {rows:?} in {source:?}"
    );
}

// —— The setter-twin read credit ————————————————————————————————————————

/// `set _p` alone + `C()._p` read — the getter miss reports
/// `undefined_getter`, but the oracle's `members` set carries the setter
/// element: no `unused_element` row.
#[test]
fn setter_only_property_read_spares_the_setter() {
    let report = report_ops(
        "class C { set _p(int v) {} }\n\
         void main() { C()._p; }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_p")),
        "setter reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The compound-assignment twin: `C()._p += 1` on a setter-only property
/// needs both faces — the getter miss reports `undefined_getter`, the
/// setter binds and stays used.
#[test]
fn setter_only_property_compound_read_spares_the_setter() {
    let report = report_ops(
        "class C { set _p(int v) {} }\n\
         void main() { C()._p += 1; }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_p")),
        "setter reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The getter+setter pair's read binds the GETTER only — `set _p` never
/// enters `usedElements.members` and reports on its own site.
#[test]
fn getter_setter_pair_read_reports_the_setter() {
    let source = "class C { int get _p => 0; set _p(int v) {} }\n\
                  void main() { C()._p; }";
    let report = report_ops(source);
    let (offset, _) = at(source, "_p(int v)", 0);
    let rows = ue_rows(&report);
    assert!(
        rows.iter()
            .any(|(o, text)| *o == offset && text.contains("_p")),
        "setter missing at {offset}: {rows:?} in {source:?}"
    );
    assert!(
        !rows.iter().any(|(o, _)| *o != offset),
        "extra rows: {rows:?} in {source:?}"
    );
}

/// The write mirror: `C()._p = 1` binds the SETTER only — `get _p`
/// reports.
#[test]
fn getter_setter_pair_write_reports_the_getter() {
    let source = "class C { int get _p => 0; set _p(int v) {} }\n\
                  void main() { C()._p = 1; }";
    let report = report_ops(source);
    let (offset, _) = at(source, "_p =>", 0);
    let rows = ue_rows(&report);
    assert!(
        rows.iter()
            .any(|(o, text)| *o == offset && text.contains("_p")),
        "getter missing at {offset}: {rows:?} in {source:?}"
    );
}

// —— The implicit-this setter write rescue ——————————————————————————————

/// A bare `_s = 1` inside the class resolves the `_s=` member — the
/// write credits the setter element even though `member_refs` never saw
/// the `.`-member edge.
#[test]
fn bare_this_setter_write_spares_the_setter() {
    let report = report_ops(
        "class C {\n\
         \x20 set _s(int v) {}\n\
         \x20 void m() { _s = 1; }\n\
         }\n\
         void main() { C().m(); }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_s")),
        "setter reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The top-level twin: a bare `_s = 1` inside a function resolves the
/// top-level `set _s` — the write credits it like any top-level use.
#[test]
fn bare_top_level_setter_write_spares_the_setter() {
    let report = report_ops(
        "set _s(int v) {}\n\
         void main() { _s = 1; }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_s")),
        "setter reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

// —— The write-only top-level exclusion —————————————————————————————————

/// `int _v = 0` + `_v = 3` alone — `_isReadIdentifier` (upstream :429)
/// credits the SETTER element, never the variable: a write-only
/// top-level reports.
#[test]
fn write_only_top_level_variable_reports() {
    let source = "int _v = 0;\n\
                  void main() { _v = 3; }";
    let report = report_ops(source);
    let rows = ue_rows(&report);
    assert!(
        rows.iter().any(|(_, text)| text.contains("_v")),
        "variable missing: {rows:?} in {source:?}"
    );
}

/// A read alongside the write spares the variable — the resolved `Decl`
/// ref without a write-intent twin joins `top_level_used`.
#[test]
fn read_and_write_top_level_variable_is_used() {
    let report = report_ops(
        "int _v = 0;\n\
         void main() { _v = 3; print(_v); }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_v")),
        "variable reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

// —— The local-shadow element check ————————————————————————————————————

/// A local `_f` shadowing the top-level `_f` — the local's uses credit
/// `LocalFunctionElement`, never the top-level decl: `top_level_used`
/// joins `RefTarget::Decl` resolutions only, so the shadowed top-level
/// reports.
#[test]
fn local_shadowed_top_level_reports() {
    let source = "int _f() => 0;\n\
                  void main() {\n\
                  \x20 int _f() => 1;\n\
                  \x20 _f();\n\
                  }";
    let report = report_ops(source);
    let (offset, _) = at(source, "_f", 0);
    let rows = ue_rows(&report);
    assert!(
        rows.iter()
            .any(|(o, text)| *o == offset && text.contains("_f")),
        "top-level missing at {offset}: {rows:?} in {source:?}"
    );
}

// —— The member_refs invoke union ———————————————————————————————————————

/// `C()._m()` — an Invoke intent never enters the field-read index; the
/// `member_refs` edge is the resolved-member pool's invoke arm. The
/// invoked member stays used.
#[test]
fn invoked_private_member_is_used() {
    let report = report_ops(
        "class C { int _m() => 0; }\n\
         void main() { C()._m(); }",
    );
    let rows = ue_rows(&report);
    assert!(
        !rows.iter().any(|(_, text)| text.contains("_m")),
        "member reported: {rows:?} in {:?}",
        all_codes(&report)
    );
}

/// The negative control: an unreferenced `int _m()` reports on its own
/// site.
#[test]
fn unreferenced_private_method_reports() {
    let source = "class C { int _m() => 0; }\n\
                  void main() {}";
    let report = report_ops(source);
    let rows = ue_rows(&report);
    assert!(
        rows.iter().any(|(_, text)| text.contains("_m")),
        "member missing: {rows:?} in {source:?}"
    );
}
