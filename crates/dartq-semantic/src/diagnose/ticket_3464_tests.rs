//! The #3464 pins: `duplicate_constructor` head-shape semantics —
//! `member.name?.lexeme ?? 'new'` keyed per container ELEMENT (the
//! `augment` fragment chain shares it), each duplicate reporting at its
//! `ConstructorDeclarationImpl.errorRange` (the type name through the
//! selector, or the `new`/`factory` keyword through the name for a
//! `new`-head/bare-factory spelling).
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) — spans
//! mirrored from `member_duplicate_definition_verifier.dart:90-130` and
//! `ast.dart`'s `ConstructorDeclarationImpl.errorRange` (:7066-7071).
//! Upstream skip: `member.typeName != firstFragment.name` heads never
//! enter the pass (`A.foo` inside `B`, `new.named`, `factory.someName`),
//! and `member.augmentKeyword` members merge into the introductory
//! element instead of declaring.

use super::super::test_rig::report_ops_experiments_with;
use crate::diagnose::FileReport;
use dartq_language::Feature;

/// Diagnoses `source` (prelude ahead) with the `primary-constructors`
/// and `augmentations` experiments on — the `new`-head, header-primary
/// and `augment` fragment shapes the pass keys on live behind them.
fn report(source: &str) -> FileReport {
    report_ops_experiments_with(
        &format!("{}{source}", super::super::test_rig::CORE_PRELUDE),
        &[],
        None,
        &[Feature::PrimaryConstructors, Feature::Augmentations],
    )
}

/// The `duplicate_constructor` rows of `source` as `(offset, length,
/// message)` tuples — offsets translated back past the prelude so they
/// address the fixture; the unnamed face carries its own message.
fn dups(source: &str) -> Vec<(u32, u32, String)> {
    let shift = super::super::test_rig::CORE_PRELUDE.len() as u32;
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_constructor")
        .map(|d| {
            (
                d.range.offset.saturating_sub(shift),
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// The `(offset, length)` of `needle`'s `nth` occurrence in `haystack`
/// (1-based — a span pin's expected position).
fn span(haystack: &str, needle: &str, nth: usize) -> (u32, u32) {
    let mut at = 0usize;
    for _ in 0..nth {
        at = haystack[at..]
            .find(needle)
            .map(|found| at + found)
            .unwrap_or_else(|| panic!("{needle:?} not found in {haystack:?}"))
            + 1;
    }
    (at as u32 - 1, needle.len() as u32)
}

const UNNAMED: &str = "The unnamed constructor is already defined.";

fn named(name: &str) -> String {
    format!("The constructor with name '{name}' is already defined.")
}

#[test]
fn ticket_3464_two_explicit_unnamed_dedupe() {
    // `C() / C()` — the plain-head arm: the second `C` reports the
    // unnamed face at the type name alone (errorRange's `typeName`
    // arm — the name slot is null).
    let source = "class C { C() {} C() {} }";
    let (offset, length) = span(source, "C", 3);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_new_head_unnamed_dedupe() {
    // `new() / new()` — the `new`-keyword head: `(` is the name slot's
    // null spelling, the range is `new` alone.
    let source = "class C { new(); new(); }";
    let (offset, length) = span(source, "new", 2);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_new_head_named_dedupe() {
    // `new named() / new named()` — `newKeyword` through the name.
    let source = "class C { new named(); new named(); }";
    let (offset, length) = span(source, "new named", 2);
    assert_eq!(dups(source), vec![(offset, length, named("named"))]);
}

#[test]
fn ticket_3464_named_primary_seeds_the_set() {
    // `class C.foo()` primary claims `foo` — the body's `C.foo` reports
    // at the type name through the selector.
    let source = "class C.foo() { C.foo() {} }";
    let (offset, length) = span(source, "C.foo", 2);
    assert_eq!(dups(source), vec![(offset, length, named("foo"))]);
}

#[test]
fn ticket_3464_named_primary_leaves_unnamed_free() {
    // `class C.named()` + `C()`: different keys — no row.
    let source = "class C.named() { C() {} }";
    assert!(dups(source).is_empty());
}

#[test]
fn ticket_3464_unnamed_primary_dedupes_secondary() {
    // `class C()` primary claims `new` — the body's `C()` reports at
    // the type name alone.
    let source = "class C() { C() {} }";
    let (offset, length) = span(source, "C", 2);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

// The three extension-type pins encode the oracle-verified verdicts
// (`dart analyze`: `extension type E(int i) { E() {} }` reports
// `duplicate_constructor` at 1:27 alongside
// `final_not_initialized_constructor`). They sit ignored: the
// `flush_init_instance_fields` call inside
// `end_extension_type_declaration` (diagnose/listener.rs — outside this
// lane's scope) gates on `constructors.len() >
// usize::from(!has_unnamed_constructor)`, which is false for every
// unnamed-only or `.new` secondary — the flush never runs, so neither
// face emits. The gate should count declared generative secondaries
// (upstream `addConstructors` walks every member ConstructorDeclaration
// unconditionally; `_addConstructor` self-filters factory/redirect/
// external). Re-verified 2026-11-01 on dart 3.35.0.

#[test]
#[ignore = "blocked by the extension-type flush gate in listener.rs (see header)"]
fn ticket_3464_extension_type_primary_unnamed() {
    // `extension type E(int i)` — the representation primary claims
    // `new`; the body's `E()` reports at the type name alone.
    let source = "extension type E(int i) { E() {} }";
    let (offset, length) = span(source, "E", 2);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
#[ignore = "blocked by the extension-type flush gate in listener.rs (see header)"]
fn ticket_3464_extension_type_primary_named() {
    // `extension type E.n(int i)` claims `n`; `E.n` reports at `E.n`.
    let source = "extension type E.n(int i) { E.n() {} }";
    let (offset, length) = span(source, "E.n", 2);
    assert_eq!(dups(source), vec![(offset, length, named("n"))]);
}

#[test]
#[ignore = "blocked by the extension-type flush gate in listener.rs (see header)"]
fn ticket_3464_extension_type_dot_new_is_the_unnamed_face() {
    // `extension type E.new(int i)` claims `new`; the body's `E.new`
    // reports the unnamed face at `E.new`.
    let source = "extension type E.new(int i) { E.new() {} }";
    let (offset, length) = span(source, "E.new", 2);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_bare_factory_dedupes_at_factory_keyword() {
    // `new(); factory()` — the bare factory head keys `new`, its range
    // the `factory` keyword alone (no typeName, no name).
    let source = "class C { new(); factory() => throw 0; }";
    let (offset, length) = span(source, "factory", 1);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_factory_named_head_spans_factory_through_name() {
    // `factory name()` duplicates `new name()` — the range is `factory`
    // through `name`.
    let source = "class C { new name(); factory name() => throw 0; }";
    let (offset, length) = span(source, "factory name", 1);
    assert_eq!(dups(source), vec![(offset, length, named("name"))]);
}

#[test]
fn ticket_3464_factory_type_name_head_spans_the_type_name() {
    // `factory C()` — `C` is a typeName: the range is `C` alone, keying
    // the unnamed face against `C()`.
    let source = "class C { C() {} factory C() => throw 0; }";
    let (offset, length) = span(source, "C", 3);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_wrong_type_name_stays_out() {
    // `A.foo()` inside `B` — `typeName != firstFragment.name` skips the
    // member outright; `B.foo` claims `foo` undisturbed.
    let source = "class A {} class B { A.foo() {} B.foo() {} }";
    assert!(dups(source).is_empty());
}

#[test]
fn ticket_3464_dot_new_head_is_a_type_name_not_new_keyword() {
    // `new.new()` — the `newConstructorDotName` recovery names the
    // constructor `new` (the `new` before the dot is the keyword, the one
    // after it the name slot), so it claims the unnamed face like `new()`
    // and the following `new()` reports at its own keyword alone.
    let source = "class C { new.new() {} new() {} }";
    let (offset, length) = span(source, "new", 3);
    assert_eq!(dups(source), vec![(offset, length, UNNAMED.into())]);
}

#[test]
fn ticket_3464_augment_member_merges_never_declares() {
    // `augment factory` is a fragment of the same element — never a
    // second declaration of `new`.
    let source = "class C { C() {} } augment class C { augment factory C() => throw 0; }";
    assert!(dups(source).is_empty());
}

#[test]
fn ticket_3464_augment_fragment_member_sees_introductory_names() {
    // A non-augment member inside an `augment` fragment is a real
    // declaration in the same element context — `C.foo` collides with
    // the introductory `C.foo`.
    let source = "class C { C.foo() {} } augment class C { C.foo() {} }";
    let (offset, length) = span(source, "C.foo", 2);
    assert_eq!(dups(source), vec![(offset, length, named("foo"))]);
}

#[test]
fn ticket_3464_plain_same_name_declaration_starts_a_fresh_set() {
    // Two plain `C` declarations are separate elements — no
    // cross-declaration `duplicate_constructor` (the `duplicate_definition`
    // face owns that row).
    let source = "class C { C() {} } class C { C() {} }";
    assert!(dups(source).is_empty());
}
