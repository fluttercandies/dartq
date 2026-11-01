//! The #3443 pins — the undefined-ARGUMENT is-face folds through the
//! lattice like the oracle's `InterfaceType` carrying `InvalidType`.
//!
//! # The wrong fact
//!
//! `tested_arg_undefined` vetoed every is-question whose tested head was a
//! registered container while some node of its written-argument tree named
//! an undefined id: `List<int> x is List<Missing>` kept BOTH arms live.
//! The official builds `List<InvalidType>` for that spelling
//! (`named_type_resolver.dart`'s per-position fill — still an
//! `InterfaceType`), and subtype.dart's 0c arm makes `S <: InvalidType`
//! and `InvalidType <: S` vacuously true at EVERY argument depth, so the
//! pinned 3.13.7 oracle answers `int <: Invalid` inside
//! `arguments_subtype` and deads the else (`ifFalseIsUnreachable`,
//! flow_analysis.dart:4156-4160). dartq's mapping already produces the
//! oracle's shape — `map_node` interns `Invalid` at the undefined
//! argument's own position — so the veto was the only thing standing
//! between the lattice and the fold.
//!
//! # The contract
//!
//! - The veto retires everywhere: `factor_typed`, `is_subtype_typed`, and
//!   `try_promote_to_typed` all answer the instantiated face. A
//!   `Box<Missing>` tested spelling now folds exactly like a whole-face
//!   `Missing` did after #3427 — `factor(_, Box<Invalid>)` is `Never` when
//!   the operand subtypes the face, `is_subtype` is vacuous through the
//!   argument's 0c arm, and the promotion gate still refuses the step
//!   (`Box<Invalid>` and the operand are mutual subtypes, so no strict
//!   step stands).
//! - A RESOLVED argument that is genuinely incompatible still fails the
//!   subtype check: `List<int> is List<String>` folds nothing.
//! - The `#2636` fuel-trip arm remains the only degradation guard — a
//!   tripped mapping under-reports (`false`/`from`/`None`), never
//!   over-folds.
//!
//! Oracle provenance: `/tmp/probe3443/*.dart` against `dart analyze`
//! 3.13.1 (`--format=machine`) — every fold below carries its machine row.

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The `dead_code` rows of `source`'s report as `(offset, length)` pairs.
/// Ranges are absolute into the mounted unit (`CORE_PRELUDE + source`), so
/// `else_start` below adds the prelude's length to the fixture's own
/// `find` result before comparing.
fn dead(source: &str) -> (Vec<(u32, u32)>, FileReport) {
    let report = report_ops(source);
    let dead = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    (dead, report)
}

/// The fixture's `needle` offset rebased onto the mounted unit's buffer.
fn at(source: &str, needle: &str) -> u32 {
    CORE_PRELUDE.len() as u32 + source.find(needle).unwrap() as u32
}

/// `List<int> x` tested `is List<Missing>`: `int <: Invalid` answers
/// `true` inside `arguments_subtype` (subtype.dart's 0c arm at the
/// argument's own position), so `factor(List<int>, List<Invalid>)` is
/// `Never` and the else arm deads — the oracle reports
/// `non_type_as_type_argument` on `Missing`, `unnecessary_type_check`
/// (always 'true') on the whole test, and `dead_code` on the else.
#[test]
fn t3443_is_list_missing_deads_the_else() {
    let source = "void f(List<int> x) {\n\
                      \x20 if (x is List<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    let else_start = at(&source, "else");
    assert_eq!(
        dead.len(),
        1,
        "exactly the else arm carries the dead row: {:#?}",
        rows(&report)
    );
    assert!(
        dead[0].0 > else_start,
        "the dead row anchors inside the else arm: {:#?}",
        rows(&report)
    );
    let names = code_names(&report);
    assert!(
        names.iter().any(|n| *n == "non_type_as_type_argument"),
        "the argument miss reports its own code: {names:?}"
    );
    assert!(
        names.iter().any(|n| *n == "unnecessary_type_check"),
        "the constant-true test reports the check: {names:?}"
    );
}

/// The `is!` polarity: the same vacuous argument subtype answers the test
/// `true`, so `is!` is constant-false and the THEN arm deads.
#[test]
fn t3443_is_not_list_missing_deads_the_then() {
    let source = "void f(List<int> x) {\n\
                      \x20 if (x is! List<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    let else_start = at(&source, "else");
    assert_eq!(
        dead.len(),
        1,
        "exactly the then arm carries the dead row: {:#?}",
        rows(&report)
    );
    assert!(
        dead[0].0 < else_start,
        "the dead row anchors inside the then arm: {:#?}",
        rows(&report)
    );
}

/// Both argument positions of a two-parameter container tolerate the
/// undefined spelling independently: `Map<int, int> is Map<Missing, int>`
/// and `Map<int, int> is Map<int, Missing>` each fold — the oracle deads
/// the else of both.
#[test]
fn t3443_map_argument_positions_fold_independently() {
    for test in ["Map<Missing, int>", "Map<int, Missing>"] {
        let source = format!(
            "void f(Map<int, int> x) {{\n\
             \x20 if (x is {test}) {{\n\
             \x20   x;\n\
             \x20 }} else {{\n\
             \x20   x;\n\
             \x20 }}\n\
             }}"
        );
        let (dead, report) = dead(&source);
        let else_start = at(&source, "else");
        assert_eq!(
            dead.len(),
            1,
            "{test}: exactly the else arm carries the dead row: {:#?}",
            rows(&report)
        );
        assert!(
            dead[0].0 > else_start,
            "{test}: the dead row anchors inside the else arm: {:#?}",
            rows(&report)
        );
    }
}

/// The vacuous arm reaches into nested argument trees:
/// `List<List<int>> is List<List<Missing>>` answers `List<int> <:
/// List<Invalid>` at depth one, whose own argument is `int <: Invalid` —
/// the else deads.
#[test]
fn t3443_nested_argument_tree_folds() {
    let source = "void f(List<List<int>> x) {\n\
                      \x20 if (x is List<List<Missing>>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    let else_start = at(&source, "else");
    assert_eq!(
        dead.len(),
        1,
        "exactly the else arm carries the dead row: {:#?}",
        rows(&report)
    );
    assert!(
        dead[0].0 > else_start,
        "the dead row anchors inside the else arm: {:#?}",
        rows(&report)
    );
}

/// The registered-head argument face over an UNRELATED operand keeps both
/// arms: `Box<Missing>` maps to `Box<Invalid>` — an interface `Base` does
/// not subtype — so nothing folds. The oracle's `List<int> is Box<Missing>`
/// (unregistered `Box`) deads through the whole-face `InvalidType`; the
/// registered-but-unrelated face below is the shape that must NOT fold.
#[test]
fn t3443_unrelated_registered_head_keeps_both_arms() {
    let source = "class Base {} class Box<T> {}\n\
                      void f(Base x) {\n\
                      \x20 if (x is Box<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    assert!(
        dead.is_empty(),
        "no arm folds over an unrelated head: {:#?}",
        rows(&report)
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|n| *n == "non_type_as_type_argument"),
        "the argument miss still reports its own code: {:#?}",
        rows(&report)
    );
}

/// The same spelling over a matching operand folds: `Box<int> x is
/// Box<Missing>` — `int <: Invalid` inside `arguments_subtype` — is the
/// direct proof the veto retired.
#[test]
fn t3443_registered_box_argument_face_folds() {
    let source = "class Box<T> {}\n\
                      void f(Box<int> x) {\n\
                      \x20 if (x is Box<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    let else_start = at(&source, "else");
    assert_eq!(
        dead.len(),
        1,
        "exactly the else arm carries the dead row: {:#?}",
        rows(&report)
    );
    assert!(
        dead[0].0 > else_start,
        "the dead row anchors inside the else arm: {:#?}",
        rows(&report)
    );
}

/// The non-regression pin: a RESOLVED argument that is genuinely
/// incompatible fails the subtype check — `List<int> is List<String>`
/// answers `int <: String` = `false` and folds nothing (no dead arm, no
/// unnecessary-type-check row).
#[test]
fn t3443_wrong_resolved_argument_folds_nothing() {
    let source = "void f(List<int> x) {\n\
                      \x20 if (x is List<String>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    assert!(
        dead.is_empty(),
        "a genuinely incompatible argument keeps both arms live: {:#?}",
        rows(&report)
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|n| *n == "unnecessary_type_check"),
        "no constant verdict rides a live test: {:#?}",
        rows(&report)
    );
}

/// The whole-face face is unchanged: `int x is Missing` still deads the
/// else and still reports `type_test_with_undefined_name` on the spelling
/// (the #3427 posture — the undefined head was never this veto's face).
#[test]
fn t3443_undefined_head_fold_unchanged() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    let else_start = at(&source, "else");
    assert_eq!(
        dead.len(),
        1,
        "exactly the else arm carries the dead row: {:#?}",
        rows(&report)
    );
    assert!(
        dead[0].0 > else_start,
        "the dead row anchors inside the else arm: {:#?}",
        rows(&report)
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|n| *n == "type_test_with_undefined_name"),
        "the head miss reports its own code: {:#?}",
        rows(&report)
    );
}

/// A nullable operand keeps the else alive: `List<int>? is List<Missing>`
/// factors to a `Null` remainder (`Null <: List<Invalid>` fails, so the
/// null half of the operand survives the test) — the oracle reports the
/// check unnecessary yet deads nothing.
#[test]
fn t3443_nullable_operand_keeps_the_else() {
    let source = "void f(List<int>? x) {\n\
                      \x20 if (x is List<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    assert!(
        dead.is_empty(),
        "the Null remainder keeps the else arm live: {:#?}",
        rows(&report)
    );
}

/// A `dynamic` operand folds nothing: `dynamic <: List<Invalid>` fails the
/// left-top rule, so both arms stay live — the oracle reports only the
/// argument miss.
#[test]
fn t3443_dynamic_operand_keeps_both_arms() {
    let source = "void f(dynamic x) {\n\
                      \x20 if (x is List<Missing>) {\n\
                      \x20   x;\n\
                      \x20 } else {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let (dead, report) = dead(source);
    assert!(
        dead.is_empty(),
        "a dynamic operand keeps both arms live: {:#?}",
        rows(&report)
    );
}
