//! The #3603 pins — the `dot_shorthand_undefined_member` message follows
//! the shorthand's own face.
//!
//! # The observed divergence
//!
//! Row-level census: of the 329 EXTRA and 430 MISSING rows, 142 pairs
//! (= 284 rows) differ in MESSAGE ONLY — the position and span already
//! match, the rendered text does not. The oracle splits the shared
//! `dot_shorthand_undefined_member` code by the shorthand's face
//! (analyzer `messages.yaml` :4927/:4981, one `sharedName`, two
//! problemMessages):
//!
//! - an invocation-position shorthand (`.bar()`) renders "The static
//!   method or constructor …" (`dotShorthandUndefinedInvocation`);
//! - a read-position shorthand (`.bar`) renders "The static getter …"
//!   (`dotShorthandUndefinedGetter`).
//!
//! dartq rendered the getter text in every position.
//!
//! # The contract
//!
//! - The message's head word follows the selector's written spelling: a
//!   selector carrying an argument list reads the invocation face, a
//!   bare selector the getter face.
//! - Both faces share the registry name `dot_shorthand_undefined_member`
//!   and the row lands at the selector span unchanged.
//!
//! Oracle provenance: `dart analyze` 3.13.7 `--format=machine` over
//! `class C {}` + `C c = .bar();` / `C c = .bar;` — the two texts above
//! at the same selector span, `DOT_SHORTHAND_UNDEFINED_MEMBER` both.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` row as `((offset, end), message)`
/// — the pins assert one miss per fixture at its selector span.
fn shorthand_row(report: &FileReport) -> ((u32, u32), String) {
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dot_shorthand_undefined_member")
        .unwrap_or_else(|| panic!("no shorthand row: {:?}", rows(report)));
    (
        (row.range.offset, row.range.end()),
        row.message.as_str().to_string(),
    )
}

// —— pin: an invocation-position shorthand renders the method-or-
// —— constructor face (oracle: `The static method or constructor 'bar'
// —— isn't defined for the context type 'C'.`) at the selector span.
#[test]
fn invocation_shorthand_reads_method_or_constructor_face() {
    let source = "class C {}\nvoid f() { C c = .bar(); }\n";
    let r = report_ops(source);
    let (span, message) = shorthand_row(&r);
    assert_eq!(
        span,
        (
            at(source, "bar"),
            at(source, "bar") + u32::try_from("bar".len()).expect("fits")
        ),
        "the selector span is unchanged: {:?}",
        rows(&r)
    );
    assert_eq!(
        message,
        "The static method or constructor 'bar' isn't defined for the context type 'C'.",
        "the invocation face's verbatim oracle text: {:?}",
        rows(&r)
    );
}

// —— pin: a read-position shorthand renders the getter face (oracle:
// —— `The static getter 'bar' isn't defined for the context type 'C'.`)
// —— at the same selector span shape.
#[test]
fn read_shorthand_reads_getter_face() {
    let source = "class C {}\nvoid f() { C c = .bar; }\n";
    let r = report_ops(source);
    let (span, message) = shorthand_row(&r);
    assert_eq!(
        span,
        (
            at(source, "bar"),
            at(source, "bar") + u32::try_from("bar".len()).expect("fits")
        ),
        "the selector span is unchanged: {:?}",
        rows(&r)
    );
    assert_eq!(
        message,
        "The static getter 'bar' isn't defined for the context type 'C'.",
        "the getter face's verbatim oracle text: {:?}",
        rows(&r)
    );
}
