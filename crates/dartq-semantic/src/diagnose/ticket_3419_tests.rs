//! #3419 pins — the anonymous-method body's `this` and scope bind the
//! RECEIVER, not the enclosing container.
//!
//! # The contract
//!
//! Upstream binds the body in the resolver (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:1952-2087
//! `visitAnonymousMethodInvocation`): `targetType =
//! realTarget.staticType ?? Object?` (promoted to non-null under
//! `?.`/`?..`), and with NO explicit formal list `_thisType =
//! parameterType` wraps the whole `body.resolve(...)` — every bare
//! identifier the enclosing scope cannot answer lexically resolves as an
//! implicit `this.` over the receiver, and every explicit `this` keyword
//! reads the receiver too, in ANY enclosing context (upstream
//! error_verifier.dart:400-406 swaps the this-context to
//! `instanceMemberBody`, so a static method, a field initializer, or a
//! constructor initializer keeps the body's `this` valid). A bare name
//! the enclosing class-family element DECLARES as an instance member
//! ends the lexical search with nulls (scope.dart's `InstanceScope
//! lookup`) — it is an implicit `this.` on the receiver, never the
//! enclosing member; a STATIC member keeps the lexical win. With an
//! explicit formal list, the receiver's static type injects into the
//! first plain unannotated formal (resolver.dart:1988-1997, regardless
//! of the formal count — extra formals keep meaningful analysis). A
//! BLOCK body's `return` statements check against the body's own
//! INFERRED return type (error_verifier.dart:408-425 swaps the enclosing
//! executable context), never the enclosing function's.
//!
//! # The defect
//!
//! All four census families were one root cause: the body resolved
//! against the ENCLOSING scope — 85 extra `undefined_*` (the receiver's
//! members reported missing), 52 `invalid_reference_to_this` (the body's
//! `this` judged by the enclosing container's context), 84
//! `return_of_invalid_type` (the body's returns checked against the
//! enclosing function), and the injection-less first formal.
//!
//! # Oracle verification
//!
//! Every expectation below is oracle-shaped against the contract above:
//! receiver-member reads silent in implicit and explicit `this` form;
//! `this` valid in static/field-initializer/constructor-initializer
//! contexts; block-body returns silent against a `void` enclosing
//! function; `?.`/`?..` bodies silent over a nullable receiver; the
//! injected formal typing `r.nope` as a getter miss on the receiver; and
//! the enclosing instance member REBINDING — a receiver without the
//! member reports `undefined_identifier` where the pre-fix engine read
//! the enclosing field silently.
//!
//! Known residual (separate root cause, out of scope): the
//! `wrong_number_of_type_arguments` family of the same census — the
//! `expectStaticType<Exactly<T>>` generic extension tear-off shape
//! present in non-anonymous conformance cases too.

use crate::diagnose::test_rig::report_ops_experiments_with;
use crate::diagnose::types::FileReport;
use dartq_language::Feature;

/// Diagnoses `source` with the anonymous-methods experiment opted in
/// (the corpus `analyzer: enable-experiment: [anonymous-methods]`
/// channel — the feature is not language-version enabled in 3.13).
/// Unlike the prelude-carrying `report_ops`, the experiments rig mounts
/// the raw source: fixture offsets index the fixture text itself.
fn anon_report(source: &str) -> FileReport {
    report_ops_experiments_with(source, &[], None, &[Feature::AnonymousMethods])
}

/// The #3419 family rows — the codes the ticket's census families count —
/// as `(code, offset, written text at the row)`; side rows of the rig
/// (unused locals, literal noise) are other faces and stay out of these
/// pins.
fn family_rows(source: &str) -> Vec<(&'static str, u32, String)> {
    let report = anon_report(source);
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_identifier"
                    | "undefined_getter"
                    | "undefined_function"
                    | "undefined_method"
                    | "invalid_reference_to_this"
                    | "return_of_invalid_type"
            )
        })
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                source[d.range.offset as usize..d.range.end() as usize].to_string(),
            )
        })
        .collect()
}

/// Offset of `needle` inside the first occurrence of `line` in `source`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The implicit-`this` read and the explicit `this` keyword both resolve
/// against the receiver's members — expression and block bodies alike —
/// and the invocation's own value is the body's type (the `int` return
/// of a body summing the receiver's `int x` fields).
///
/// Pre-fix: `undefined_identifier` at the bare `x` rows and
/// `invalid_reference_to_this`-family rows through the top-level
/// function's `this`.
#[test]
fn implicit_and_explicit_this_read_the_receiver() {
    let fixture = "class C {\n  int x = 3;\n}\n\
                   int probe() {\n  \
                     C c = C();\n  \
                     c.=> x;\n  \
                     c.=> this.x;\n  \
                     return c.{ x + this.x };\n\
                   }\n";
    let rows = family_rows(fixture);
    assert_eq!(rows, Vec::new(), "{rows:#?}");
}

/// The body's `this` stays valid wherever the ENCLOSING container's
/// `this` is not: a static field initializer, a non-late field
/// initializer, and a constructor initializer (upstream swaps the
/// this-context to `instanceMemberBody` for the implicit-formals body).
///
/// Pre-fix: `invalid_reference_to_this` (static and field-initializer
/// rows) or an enclosing-container getter miss.
#[test]
fn static_and_initializer_contexts_keep_this_valid_on_the_receiver() {
    let fixture = "class C {\n  int x = 3;\n}\n\
                   class A {\n  \
                     static int a = C().=> this.x;\n  \
                     int b = C().=> this.x;\n  \
                     int g = 0;\n  \
                     A() : g = C().=> this.x {}\n\
                   }\n";
    let rows = family_rows(fixture);
    assert_eq!(rows, Vec::new(), "{rows:#?}");
}

/// A BLOCK body's `return` checks against the body's own inferred return
/// type — never the enclosing function's `void`.
///
/// Pre-fix: `return_of_invalid_type` (`A value of type 'int' can't be
/// returned ...`), the census's 84-row family.
#[test]
fn a_block_body_return_checks_only_the_inferred_body_type() {
    let fixture = "class C {\n  int x = 3;\n}\n\
                   void probe() {\n  \
                     C().{ return 1; };\n  \
                     C c = C();\n  \
                     c.{ return this.x; };\n\
                   }\n";
    let rows = family_rows(fixture);
    assert_eq!(rows, Vec::new(), "{rows:#?}");
}

/// `?.` and `?..` promote the receiver for the body's binding: a
/// nullable receiver's members read without the unchecked-access face,
/// through the bare implicit-`this` form and the explicit `this` alike.
#[test]
fn null_aware_selectors_promote_the_this_binding() {
    let fixture = "class C {\n  int x = 3;\n}\n\
                   void probe() {\n  \
                     C? c = C();\n  \
                     c?.=> x;\n  \
                     c?.=> this.x;\n  \
                     c?..{ this.x; };\n\
                   }\n";
    let rows = family_rows(fixture);
    assert_eq!(rows, Vec::new(), "{rows:#?}");
}

/// The explicit formal list's first plain positional formal carries the
/// receiver's static type: its member reads type exactly like the
/// receiver's own — `r.nope` is a getter miss ON THE RECEIVER — while the
/// resolved `r.x` stays silent. The `this` keyword is the ENCLOSING
/// context's (resolver.dart:2053-2055 resolves the explicit-formals body
/// with no `_thisType` swap — only the implicit form rebind it), so
/// `this.x` inside the class `A` body reads A's own `x` and stays silent.
///
/// Pre-fix: the formal read as untyped and `r.nope` fell silent.
#[test]
fn the_first_formal_carries_the_receiver_type() {
    let fixture = "class C {\n  int x = 3;\n}\n\
                   class A {\n  \
                     int x = 1;\n  \
                     void probe() {\n    \
                       C c = C();\n    \
                       c.(r) => r.x + this.x;\n    \
                       c.(r) => r.nope;\n  \
                     }\n\
                   }\n";
    let rows = family_rows(fixture);
    assert_eq!(
        rows,
        [(
            "undefined_getter",
            at(fixture, "c.(r) => r.nope;", "nope"),
            "nope".to_string(),
        )],
        "{rows:#?}"
    );
}

/// An enclosing class-family element's own INSTANCE member does NOT win
/// the body's bare name — the lexical search ends in nulls and the name
/// resolves against the RECEIVER: a receiver WITHOUT the member reports
/// `undefined_identifier`, a receiver WITH it stays silent (rebinding
/// away from the enclosing field, whose `int x` the pre-fix engine read
/// silently on both rows).
#[test]
fn an_enclosing_instance_member_rebinds_to_the_receiver() {
    let fixture = "class A {\n  int x = 1;\n  \
                     void probe() {\n    \
                       C().{ x; };\n    \
                       D().{ x; };\n  \
                     }\n\
                   }\n\
                   class C {}\n\
                   class D {\n  int x = 5;\n}\n";
    let rows = family_rows(fixture);
    assert_eq!(
        rows,
        [(
            "undefined_identifier",
            at(fixture, "C().{ x; };", "x"),
            "x".to_string(),
        )],
        "{rows:#?}"
    );
}
