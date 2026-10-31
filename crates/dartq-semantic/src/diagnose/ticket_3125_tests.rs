//! The #3125 pins: the member-miss cascade/bound faces —
//! `undefined_getter`/`undefined_method`/`undefined_setter` over cascade
//! selectors and a `T`-spelled receiver inside a generic scope.
//!
//! Cascade: the oracle resolves every section against `cascade.target`'s
//! static type (`type_analyzer.dart`'s `cascadeReceiver`). A continuation
//! selector's own member read may record `Receiver::Unknown` (the
//! invocation/assignment fold left it empty); the verdict still reads the
//! target's face — pinned via the newest prior `member_reads` entry that
//! still carries a real receiver, bounded at the cascade's own statement
//! (`semicolon_between`).
//!
//! Bound-T: the invalid-container bail reads parse-time
//! `type_params`/`pending_type_params`/`param_bound` — all torn down at
//! pass 2 — so a `void f<T extends num>(T t)` receiver spelled `T` used
//! to read as an invalid container. The post-walk `param_bound_of` scans
//! `exec_bounds`/`method_exec_bounds`/`container_params` windows — the
//! bound the oracle consults (`resolution_visitor.dart`'s bound
//! capture) — and keeps the member face live when the name is captured
//! as a scope parameter.

use super::test_rig::report_ops;

fn rows_named(report: &crate::diagnose::FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— cascade: `..foo` getter + `..foo()` method + `..foo = v` setter ——

/// The oracle reports each section's own miss: a `..foo` read is
/// `undefined_getter`, `..foo()` is `undefined_method`, `..foo = v` is
/// `undefined_setter` — all against the same target's static type.
#[test]
fn cascade_selector_misses_report_each_intent() {
    let report = report_ops(
        "class C { int x = 0; }\n\
         void main() { var c = C()..foo..foo()..foo = 1; }\n",
    );
    let getters = rows_named(&report, "undefined_getter");
    let methods = rows_named(&report, "undefined_method");
    let setters = rows_named(&report, "undefined_setter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert_eq!(setters.len(), 1, "one setter miss: {setters:?}");
}

/// The first section's `..foo` already emits through the latched
/// `Created`/`Typed` face — the fix keeps that arm unchanged.
#[test]
fn cascade_first_section_keeps_target_face() {
    let report = report_ops(
        "class C { int x = 0; }\n\
         void main() { var c = C()..foo = 1; }\n",
    );
    let setters = rows_named(&report, "undefined_setter");
    assert_eq!(setters.len(), 1, "one setter miss: {setters:?}");
}

/// A section's own `member_reads` entry may record `Receiver::Unknown`
/// when its fold was an invocation or an assignment — the walk-back
/// selects the newest prior entry that still carries a receiver.
#[test]
fn cascade_unknown_recv_inherits_prior_section_face() {
    let report = report_ops(
        "class C { int x = 0; }\n\
         void main() { var c = C()..foo..foo()..foo = 1; }\n",
    );
    // The three section misses all fire against the target's `C` face —
    // not `InvalidType`, not silent.
    let all = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_getter" | "undefined_method" | "undefined_setter"
            )
        })
        .map(|d| (d.code.name, d.message.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(all.len(), 3, "three section misses: {all:?}");
    assert!(
        all.iter().all(|(_, msg)| msg.contains("type 'C'")),
        "misses name the target type: {all:?}"
    );
}

/// A non-cascade unknown receiver keeps the under-report: `dynamic`
/// dispatches without a member face.
#[test]
fn unknown_recv_stays_silent_outside_cascade() {
    let report = report_ops(
        "void main() { dynamic d; d.foo; d.foo(); d.foo = 1; }\n",
    );
    let member_misses = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_getter" | "undefined_method" | "undefined_setter"
            )
        })
        .count();
    assert_eq!(member_misses, 0, "dynamic member stays silent");
}

// —— bound-T: a generic-scope `T`-spelled receiver ——

/// `T extends num`: `t.foo` resolves against the bound's interface
/// (`num` lacks `foo`) — `undefined_getter`, not silent. Pre-fix the
/// invalid-container bail fired because `is_scope_param`/`param_bound`
/// read parse-time tables torn down at pass 2; `param_bound_of` scans
/// the captured windows the bound still lives in.
#[test]
fn bound_param_receiver_reports_member_miss() {
    let report = report_ops(
        "class C<T extends num> { void m(T t) { t.foo; } }\n\
         void main() {}\n",
    );
    let getters = rows_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss on T: {getters:?}");
}

/// A bound spelling a member that EXISTS stays silent — the bound's
/// interface governs the member face, not the param name's spelling.
#[test]
fn bound_param_existing_member_stays_silent() {
    let report = report_ops(
        "class C<T extends num> { void m(T t) { t.abs(); } }\n\
         void main() {}\n",
    );
    assert!(
        rows_named(&report, "undefined_method").is_empty(),
        "num.abs exists — silent"
    );
}

/// An unbounded `T` resolves against the implicit `Object?` bound —
/// `t.noSuchMember` reports, `t.toString()`/`t.hashCode` don't.
#[test]
fn unbounded_param_uses_object_bound() {
    let report = report_ops(
        "void f<T>(T t) { t.noSuchMember; }\n\
         void main() {}\n",
    );
    let getters = rows_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "Object face still reports: {getters:?}");
}

/// A `T` receiver used from a nested executable still escapes the
/// invalid bail — `method_exec_bounds` covers the method window.
#[test]
fn method_exec_bounds_window_covers_method_scoped_T() {
    let report = report_ops(
        "class C { void m<T extends num>(T t) { t.foo; } }\n\
         void main() {}\n",
    );
    let getters = rows_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "method-scoped T: {getters:?}");
}

/// A top-level generic function's `T` (`exec_bounds` window) reports.
#[test]
fn top_level_generic_function_T_reports() {
    let report = report_ops("void f<T extends num>(T t) { t.foo; }\nvoid main() {}\n");
    let getters = rows_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "top-level generic T: {getters:?}");
}

/// The invalid-container bail still fires for a truly unknown head —
/// the new `param_bound_of` arm only exempts names a bound window
/// captured, not arbitrary spellings.
#[test]
fn unknown_container_still_silences_member_face() {
    let report = report_ops(
        "void f() { scanner.foo; }\n\
         void main() {}\n",
    );
    // `scanner` resolves nowhere — the member face stays silent; only
    // the identifier-level miss reports.
    let member_misses = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_getter" | "undefined_method" | "undefined_setter"
            )
        })
        .count();
    assert_eq!(member_misses, 0, "invalid container silences member face");
}
