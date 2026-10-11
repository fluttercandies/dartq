//! M0-gate wave-8 regression pins (#608): duplicate-emission dedup — the
//! double `missing_identifier` on a member head whose name slot holds junk,
//! and the parser-side abstract+final/interface/sealed modifier trio that
//! the semantic layer solely owns. Official decision order
//! (`parser_impl.dart`, Dart 3.15 main):
//!
//! * A missing member name recovers ONCE: `insertSyntheticIdentifier`
//!   reports through the context's recovery template
//!   (:4056-4067, `methodDeclaration` default `expectedIdentifier`) and the
//!   synthetic flows INTO the member parse as the name token
//!   (:4364-4369 insert-then-fall-through; :4398 `parseFields(...,
//!   token.next!, ...)`). The name slot must not re-report — a second
//!   `missing_identifier` at the junk token is a duplicate row (r5:
//!   `use_late_in_non_nnbd.dart:9` n=2). `parseFields` then adds exactly
//!   its own rider for the type-less, modifier-less head (:4478).
//! * The `abstract`+`final`/`interface`/`sealed` class-modifier combination
//!   is reported by the analyzer in the resolution phase
//!   (`pkg/analyzer/lib/src/dart/resolver/...` — the parser-side
//!   `parser_impl.dart` never mints these texts); in this codebase the
//!   semantic emitter (dartq-resolution `modifiers.rs`,
//!   `check_modifier_declaration`) is the sole owner because only it can
//!   carry corrections — the parser must stay silent on those heads.

use super::collector::setup;

/// Exact code stream for a member head missing its name before a `;`.
#[test]
fn late_semicolon_top_level_single_missing_identifier() {
    let mut p = setup("late;\nint x;\n");
    p.parse_unit();
    let got = p.listener.errors;
    let want = vec![
        ("missing_identifier".to_owned(), 4, 5),
        ("missing_const_final_var_or_type".to_owned(), 4, 5),
    ];
    assert_eq!(got, want, "use_late `late;` head: {got:?}");
}

/// The class-member flavor (`late;` inside a class): the official
/// `beforeStart` is captured before the modifier walk
/// (`parser_impl.dart:5295`), so in `recoverFromInvalidMember` the
/// no-progress check `token == beforeStart` (:11352) is false for a
/// modifier-headed junk member and the head flows into `parseFields`,
/// which reports its type-less rider FIRST (:4477-4480,
/// `missingConstFinalVarOrType`) and then the name-slot
/// `missingIdentifier` via `ensureIdentifierPotentiallyRecovered`
/// (:4500-4504) — never `expectedClassMember` (:11355 only fires when
/// nothing since member entry was consumed).
#[test]
fn late_semicolon_class_member_official_stream() {
    let mut p = setup("class C {\n  late;\n}\n");
    p.parse_unit();
    let got = p.listener.errors;
    let want = vec![
        ("missing_const_final_var_or_type".to_owned(), 16, 17),
        ("missing_identifier".to_owned(), 16, 17),
    ];
    assert_eq!(got, want, "class-member `late;` official stream: {got:?}");
}

/// The reserved-word name recovery (`<return type>? <reserved> <follower>`)
/// still parses and the following declaration stays clean — the synthetic
/// name hands off without a second report.
#[test]
fn missing_name_head_then_declaration_parses_clean() {
    let mut p = setup("late;\n;\nlate int x;\n");
    p.parse_unit();
    let got = p.listener.error_codes();
    assert_eq!(
        got.iter().filter(|c| *c == "missing_identifier").count(),
        1,
        "exactly one missing_identifier across the sequence: {got:?}"
    );
}

// —— family: abstract+final/interface/sealed trio ownership ————————————————

/// `abstract sealed class` is parser-legal: the combination diagnostic is
/// the semantic layer's (`modifiers.rs`), so the parser emits nothing.
#[test]
fn abstract_sealed_class_parser_silent() {
    let mut p = setup("abstract sealed class C {}\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "parser must not report the modifier trio: {:?}",
        p.listener.errors
    );
}

#[test]
fn abstract_final_interface_class_parser_silent() {
    let mut p = setup("abstract final interface class C {}\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "parser must not report the modifier trio: {:?}",
        p.listener.errors
    );
}

// —— family: named-only record type annotation in statement position ———————

/// `({int a}) t;` is a legal local declaration with a named-only record
/// type. The named-fields arm of `parse_record_type` must consume ONLY the
/// group's `)` (`ensureCloseParen` consumes it in every arm and returns its
/// span, cursor already past — the official named arm consumes
/// `token.next!` directly, parser_impl.dart:1747-1907); re-peeking after
/// that consume minted `end` from the FOLLOWING token and the trailing
/// advance swallowed the variable name, so the name slot ran on `;`
/// (missing_identifier + a synthetic empty name).
#[test]
fn named_only_record_type_local_parses_clean() {
    let mut p = setup("void main() { ({int a}) t; }\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "named-only record type local must parse clean: {:?}",
        p.listener.errors
    );
    p.listener.assert_sequence(&[
        ("begin", "record_type"),
        ("end", "record_type"),
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

/// The same consume shape with an initializer, and as a member return
/// type (the GFT record-return pass reuses the named arm).
#[test]
fn named_only_record_type_shapes_parse_clean() {
    for (source, label) in [
        (
            "void main() { ({int a}) t = (a: 1); }\n",
            "with initializer",
        ),
        ("({int a}) f() { throw ''; }\n", "member return type"),
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert!(
            p.listener.errors.is_empty(),
            "{label}: {:?}",
            p.listener.errors
        );
    }
}
