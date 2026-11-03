//! The #3370 pins — the `undefined_*` residuals the recon cluster map
//! routes to `diagnose/augmentations.rs` and `diagnose/pattern_scope.rs`.
//!
//! # What these two files actually own
//!
//! Neither file contains an `Undefined*` emit site: `augmentations.rs` is
//! the declaration verifier (`augmentation_without_declaration`,
//! `augmentation_of_*`, `declaration_already_complete`, …) and
//! `pattern_scope.rs` is pattern-scope bookkeeping whose only entry points
//! are [`super::pattern_scope`]'s `if_*` / `switch_member_*` events. Both
//! clusters therefore reach `undefined_*` rows only through the scope state
//! `pattern_scope.rs` maintains (C10/C11) — and, for the augmentation
//! cluster (C6), not at all: that one is a member-table merge gap.
//!
//! Three pins are marked `#[ignore]` because they encode the **oracle**
//! behaviour, not today's. They carry the exact repro, the expected
//! `(offset, length)` and the file that must own the fix, so the owning
//! lane flips `#[ignore]` off as its acceptance test rather than
//! re-deriving the shape:
//!
//! - `if_case_element_else_does_not_see_the_binder` — C10, oracle
//!   co19 case `LanguageFeatures/Patterns/scope_A06_t02`. Owner:
//!   `diagnose/listener.rs` (`begin_if_control_flow` /
//!   `handle_then_control_flow` / `end_if_else_control_flow` never fire
//!   `if_condition_begin`/`if_then_begin`/`if_then_end`, so an if-case
//!   **element**'s binders never close — the if-**statement** pair at
//!   `begin_then_statement`/`begin_else_statement` is the only wiring).
//! - `augment_declared_members_join_the_introduced_class` — C6, oracle
//!   co19 case `LanguageFeatures/Augmentations/scoping_A01_t01`. Owner:
//!   `collector.rs` (`Builder::member_scopes` is built per unit, so an
//!   `augment class C` in a part file opens a fresh `MemberScope` instead
//!   of joining the introduction's — the merge key must resolve across the
//!   library's units, the way upstream keys the context on the merged
//!   element's first fragment).
//! - `wildcard_type_parameter_does_not_bind` — C11, oracle
//!   co19 case `LanguageFeatures/Wildcards/binding_A01_t08`. Owner:
//!   `collector.rs`/`resolution.rs` (`resolve_bare`'s `type_params` arm;
//!   upstream a wildcard `_` declaration introduces no element).

use super::test_rig::{
    CORE_PRELUDE, code_names, report_ops, report_ops_augment_intro_experiments,
};

/// Absolute offset of `needle`'s first byte in a `report_ops` fixture (the
/// rig prepends [`CORE_PRELUDE`]).
#[track_caller]
fn at(fixture: &str, needle: &str) -> u32 {
    let inner = fixture.find(needle).unwrap_or_else(|| {
        panic!("fixture does not contain {needle:?}");
    });
    CORE_PRELUDE.len() as u32 + inner as u32
}

/// Every `undefined_*` row as `code @offset+len`, in emission order.
fn undefined_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("undefined_"))
        .map(|d| {
            format!(
                "{} @{}+{}",
                d.code.name, d.range.offset, d.range.length
            )
        })
        .collect()
}

// — green guards ———————————————————————————————————————————————————————
//
// The two shapes below are driven by the `pattern_scope.rs` state the C10
// fix will touch: an if-case binder must stay readable across a nested
// collection `if`-element, and inside its own `when` guard. Both hold
// today; they exist so the element-scope wiring cannot retire a binder that
// is still in scope.

/// An if-case binder is readable inside an `if`-element of its own then
/// block. The element path opens no case scope of its own, so the binder's
/// `var_types` slot and `locals` row are the only things answering — an
/// element close wired to the wrong edge would retire them here first.
#[test]
fn if_case_binder_reaches_an_if_element_in_its_then_branch() {
    let fixture = "void test(int x) {\n  \
                   if (x case var v) {\n    \
                   var l = [if (x > 0) v else v];\n    \
                   l;\n  \
                   }\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(undefined_rows(&report), Vec::<String>::new(), "{:?}", code_names(&report));
}

/// The `when` guard runs inside the case scope: the binder it guards is
/// already declared when the guard expression resolves (upstream
/// `GuardedPattern` — the guard's scope nests the pattern's).
#[test]
fn if_case_guard_reads_its_own_binder() {
    let fixture = "void test(int x) {\n  \
                   if (x case var v when v > 0) {\n    \
                   v;\n  \
                   }\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(undefined_rows(&report), Vec::<String>::new(), "{:?}", code_names(&report));
}

// — C10 gap: the if-case ELEMENT closes its case scope (oracle
//   `Patterns/scope_A06_t02`) ————————————————————————————————————————
//
// `if (42 case var v1) 2 else v1` inside a list literal: the else-element is
// outside the case scope, so `v1` is undefined. `pattern_scope.rs` closes an
// if-case statement's scope at `begin_else_statement`/`end_if_statement`;
// the element form fires `handle_then_control_flow` /
// `end_if_else_control_flow` instead and nothing closes, so `v1` still
// resolves. Row delta on the corpus case: 1 MISSING `undefined_identifier`
// (plus the sibling `unused_local_variable`/`dead_code` rows the close
// unlocks in `scope_A06_t02`).
#[test]
#[ignore = "C10 gap: if-case ELEMENT scope is unwired — owner diagnose/listener.rs"]
fn if_case_element_else_does_not_see_the_binder() {
    let fixture = "void test() {\n  \
                   var l = [1, if (42 case var vv) 2 else vv, 3];\n  \
                   l;\n\
                   }\n";
    let report = report_ops(fixture);
    let want = format!("undefined_identifier @{}+2", at(fixture, "else vv") + 5);
    assert_eq!(undefined_rows(&report), vec![want], "{:?}", code_names(&report));
}

// — C6 gap: augmentation members join the introduced declaration's member
//   table (oracle `Augmentations/scoping_A01_t01`) ————————————————————
//
// `class C { ... baz ... }` whose `augment class C` lives in a second unit
// of the same library: the augment's `baz` is a member of the merged `C`,
// so a body read resolves. Response shape from the corpus: 101 EXTRA rows
// (`undefined_getter`/`undefined_method` on `C.baz`, `this.qux`,
// `testIntroductory`) across `scoping_A01_t0*`, `eliminated_errors_A01_*`,
// `extensions_A01_*` and the `resolution/property_access` augment tests.
#[test]
#[ignore = "C6 gap: augment members merge per unit — owner collector.rs MemberScope key"]
fn augment_declared_members_join_the_introduced_class() {
    let main = "class C {\n  void test() {\n    baz;\n  }\n}\n";
    let aug = "augment class C {\n  static const baz = 2;\n}\n";
    let report = report_ops_augment_intro_experiments(
        main,
        aug,
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(undefined_rows(&report), Vec::<String>::new(), "{:?}", code_names(&report));
}

// — C11 gap: a wildcard declaration binds nothing (oracle
//   `Wildcards/binding_A01_t08`) ————————————————————————————————————
//
// A `_` type parameter (and every other wildcard declaration) introduces no
// element, so a body read of `_` is `undefined_identifier` — upstream
// reports it, dartq resolves it through `resolve_bare`'s `type_params` arm.
// Row delta: 18 MISSING rows across `binding_A01_t08`/`t09`.
#[test]
#[ignore = "C11 gap: wildcard declarations still bind — owner collector.rs / resolution.rs"]
fn wildcard_type_parameter_does_not_bind() {
    let fixture = "void test<_>() {\n  _;\n}\n";
    let report = report_ops(fixture);
    let want = format!("undefined_identifier @{}+1", at(fixture, "  _;") + 2);
    assert_eq!(undefined_rows(&report), vec![want], "{:?}", code_names(&report));
}
