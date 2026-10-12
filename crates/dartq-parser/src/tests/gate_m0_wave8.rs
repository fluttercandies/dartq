//! M0-gate wave-8 regression pins (#608): duplicate-emission dedup — the
//! double `missing_identifier` on a member head whose name slot holds junk,
//! and the parser-side abstract+final/interface/sealed modifier trio that
//! the semantic layer solely owns. Official decision order
//! (`parser_impl.dart`, Dart 3.15 main):
//!
//! * A missing member name recovers ONCE. The oracle split (probed
//!   `dart analyze` 3.13.1, machine format): the head insert reports
//!   `missing_identifier` only with a `var`/`final`/`const` modifier
//!   (`var ;` → one row on the `;`); a `late` or type-headed junk name
//!   (`late ;`, `int ;`, `x ;`) inserts silently and `parseFields`'s
//!   type-less rider is the only row, anchored on the member head — the
//!   `late` keyword or the type-position token, never the follower `;`.
//! * The `abstract`+`final`/`interface`/`sealed` class-modifier combination
//!   is reported by the analyzer in the resolution phase
//!   (`pkg/analyzer/lib/src/dart/resolver/...` — the parser-side
//!   `parser_impl.dart` never mints these texts); in this codebase the
//!   semantic emitter (dartq-resolution `modifiers.rs`,
//!   `check_modifier_declaration`) is the sole owner because only it can
//!   carry corrections — the parser must stay silent on those heads.

use super::collector::setup;

/// Exact stream for a member head missing its name before a `;`. Oracle
/// 3.13.1 (`dart analyze --format=machine`, `late;\nint x;\n`): the `late`
/// head recovers silently — the inserted-name face only fires with a
/// `var`/`final`/`const` modifier (`var ;` probe) — and `parseFields`'s
/// type-less rider anchors on the member head `late` (offset 0..4), not the
/// follower `;`. The earlier two-row pin predated the live probe.
#[test]
fn late_semicolon_top_level_single_missing_identifier() {
    let mut p = setup("late;\nint x;\n");
    p.parse_unit();
    let got = p.listener.errors;
    let want = vec![("missing_const_final_var_or_type".to_owned(), 0, 4)];
    assert_eq!(got, want, "use_late `late;` head: {got:?}");
}

/// The class-member flavor (`late;` inside a class): same oracle split —
/// the silent head insert and ONE type-less rider anchored on the `late`
/// keyword (probed 3.13.1: line 2 col 3, len 4), never
/// `expectedClassMember` and never a name-slot `missing_identifier`.
#[test]
fn late_semicolon_class_member_official_stream() {
    let mut p = setup("class C {\n  late;\n}\n");
    p.parse_unit();
    let got = p.listener.errors;
    let want = vec![("missing_const_final_var_or_type".to_owned(), 12, 16)];
    assert_eq!(got, want, "class-member `late;` official stream: {got:?}");
}

/// The reserved-word name recovery lane: `late;` recovers with the single
/// type-less rider, the stray `;` is the only other row, and the following
/// declaration stays clean — no inserted-name face anywhere in the unit
/// (oracle 3.13.1, `late;\n;\nlate int x;\n`: exactly these two rows).
#[test]
fn missing_name_head_then_declaration_parses_clean() {
    let mut p = setup("late;\n;\nlate int x;\n");
    p.parse_unit();
    assert_eq!(
        p.listener.error_codes(),
        ["missing_const_final_var_or_type", "unexpected_token"],
        "oracle stream for `late;` + stray `;` + clean follow-up: {:?}",
        p.listener.errors
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
