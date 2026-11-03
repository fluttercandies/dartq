//! The #3859 pins: the hop substitution of an INFERRED member face.
//!
//! `unnecessary_type_check`'s extras were dominated by one shape — a generic
//! inherited member read through a supertype hop kept the DECLARING
//! container's raw type-parameter leaf, so `x.field is A<String>` and
//! `x.field is A<int>` both read constant-true. The hop compiles its
//! substitution environment from the member's element-model signature, and
//! an inferred field (`var field = A<T>();`) has no such signature: the
//! collector runs before any inference, so `MemberEntry::var_ty` stays
//! `Unknown` and the environment never composed. The read site then folded
//! the diagnosing pass's inference overlay in AFTER the climb — still
//! spelling the declaring container's parameters.
//!
//! Oracle provenance: `dart analyze` 3.13.7. Upstream keeps the instantiated
//! executable per supertype edge (`inheritance_manager3.dart:554-571`) and
//! per applied mixin (:589-608, `_addMixinMembers` :354-371), so `M`'s
//! `A<T>` enters `C1<V>` as `A<V>`, and `getMember3` (:266-287) substitutes
//! `V := int` at `C1<int>`; the property read takes the accessor's
//! already-substituted return (`property_element_resolver.dart:588-602`).
//!
//! Every fixture below is written in its INFERRED form — the shape the
//! defect lived in. The declared twin (`A<T> field = A<T>();`) answered the
//! oracle correctly before this change, which is why the same expectation
//! holds for both; the twin is pinned beside the inferred shape wherever
//! the two must stay identical.
//!
//! The rig models no `Object` class element, so the mixin-application
//! spelling the report used (`class C1<V> = Object with M<V>;`) would pin
//! the reduced engine's unresolved-superclass recovery instead of the hop.
//! The hop is the same one under every spelling — the fixtures take the
//! class with the mixin applied (`with M<V>`) and read through a receiver
//! whose type the row carries (`C1<int> x`), which is the shape the
//! real-engine repro reaches after the hop (identical entries; the
//! declared twin's oracle-correct answer proves the equivalence).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The analyzed text and its report: the shared rig mounts [`CORE_PRELUDE`]
/// ahead of the fixture on one line, so the text the offsets index into is
/// the prelude plus the fixture verbatim.
fn analyze(fixture: &str) -> (String, super::FileReport) {
    (format!("{CORE_PRELUDE}{fixture}"), report_ops(fixture))
}

/// The 1-based line of one UTF-16 offset.
fn line_of(source: &str, offset: u32) -> u32 {
    let mut line = 1u32;
    let mut utf16 = 0u32;
    for ch in source.chars() {
        if utf16 >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
        utf16 += ch.len_utf16() as u32;
    }
    line
}

/// The 1-based line of the first occurrence of `needle`.
fn line_of_text(source: &str, needle: &str) -> u32 {
    let offset = source.find(needle).expect("the fixture spells the needle");
    line_of(source, offset as u32)
}

/// The lines carrying `code`, in emission order.
fn coded_lines(source: &str, report: &super::FileReport, code: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| line_of(source, d.range.offset))
        .collect()
}

/// The mixin hop: `class C1<V> with M<V>` applies `M` with the using
/// container's own parameter, so `M`'s inferred `A<T>` reads `A<int>` at
/// `C1<int>` — `is A<String>` is a real question, `is A<int>` is not.
#[test]
fn ticket_3859_mixin_hop_substitutes_the_inferred_field() {
    let fixture = "mixin M<T> { var field = A<T>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 var s = x.field is A<String>;\n\
                   \x20 var t = x.field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "only the provable direction reports: {:#?}",
        rows(&report)
    );
}

/// The declared twin of the same hop answers identically — the substitution
/// is the same one, reached through the model slot instead of the inferred
/// face. Divergence here is the defect itself.
#[test]
fn ticket_3859_declared_twin_matches_the_inferred_shape() {
    let fixture = "mixin M<T> { A<T> field = A<T>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 var s = x.field is A<String>;\n\
                   \x20 var t = x.field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "the declared face keeps its pre-existing answer: {:#?}",
        rows(&report)
    );
}

/// A typed receiver reads the hop's entry one instantiation further
/// (`C1<int>` → `V := int`), which only lands when the hop replaced `T`
/// with `V` first — a raw leaf surviving the hop made both polarities
/// provable here too.
#[test]
fn ticket_3859_extends_hop_substitutes_the_inferred_field() {
    let fixture = "class B<T> { var field = A<T>(); }\n\
                   class A<U> {}\n\
                   class C1 extends B<int> {}\n\
                   void main() {\n\
                   \x20 var s = C1().field is A<String>;\n\
                   \x20 var t = C1().field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "the hop resolves before the receiver does: {:#?}",
        rows(&report)
    );
}

/// The non-generic control: a hop whose member names no container
/// parameter keeps both directions as the oracle decides them — the
/// substitution must never reach a face it has nothing to replace on. Its
/// environment is `Unchanged` in both the pre- and post-fix engine.
#[test]
fn ticket_3859_non_generic_inherited_field_stays_correct() {
    let fixture = "mixin M { var field = A<int>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 var s = x.field is A<String>;\n\
                   \x20 var t = x.field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "a parameter-free face keeps its declared answer: {:#?}",
        rows(&report)
    );
}

/// The over-substitution guard: a GENERIC declaring container whose
/// inferred field writes a concrete argument (`var plain = A<int>();`)
/// mentions no parameter, so the hop substitutes nothing and the concrete
/// face stands — reading the container's parameters into it would turn the
/// `A<String>` check into a constant too.
#[test]
fn ticket_3859_parameter_free_inferred_field_is_not_substituted() {
    let fixture = "mixin M<T> { var plain = A<int>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 var s = x.plain is A<String>;\n\
                   \x20 var t = x.plain is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "only the concrete match is provable: {:#?}",
        rows(&report)
    );
}

/// The silence direction, bounded by an explicit `dynamic` argument: a
/// `C1<dynamic>` receiver's `A<dynamic>` makes BOTH checks real questions
/// (`A<dynamic> is A<String>` is informative), so neither may report. The
/// raw-leaf leak reported both — this is the polarity the constant-true
/// amplifier must never be handed.
#[test]
fn ticket_3859_dynamic_argument_receiver_keeps_both_checks_silent() {
    let fixture = "mixin M<T> { var field = A<T>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<dynamic> x = C1();\n\
                   \x20 var s = x.field is A<String>;\n\
                   \x20 var t = x.field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert!(
        coded_lines(&source, &report, "unnecessary_type_check").is_empty(),
        "a `dynamic` argument leaves both directions informative: {:#?}",
        rows(&report)
    );
}

/// The assignability face reads the same hop-substituted shape: the
/// inferred field's `A<int>` cannot fill an `A<String>` slot, and no
/// `invalid_assignment` may fire on the `A<int>` one. The raw leaf silenced
/// the mismatch entirely.
#[test]
fn ticket_3859_assignability_sees_the_substituted_inferred_field() {
    let fixture = "mixin M<T> { var field = A<T>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 A<String> z = x.field;\n\
                   \x20 A<int> w = x.field;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "invalid_assignment"),
        vec![line_of_text(&source, "A<String> z")],
        "only the mismatched slot reports: {:#?}",
        rows(&report)
    );
}

/// A declared getter return keeps its answer: the hop substitution ran on
/// the model face all along there, and the inferred-face path must not
/// disturb it.
#[test]
fn ticket_3859_declared_getter_face_is_unchanged() {
    let fixture = "mixin M<T> { A<T> get field => A<T>(); }\n\
                   class A<U> {}\n\
                   class C1<V> with M<V> {}\n\
                   void main() {\n\
                   \x20 C1<int> x = C1();\n\
                   \x20 var s = x.field is A<String>;\n\
                   \x20 var t = x.field is A<int>;\n\
                   }\n";
    let (source, report) = analyze(fixture);
    assert_eq!(
        coded_lines(&source, &report, "unnecessary_type_check"),
        vec![line_of_text(&source, "is A<int>")],
        "a declared return substitutes exactly as before: {:#?}",
        rows(&report)
    );
}
