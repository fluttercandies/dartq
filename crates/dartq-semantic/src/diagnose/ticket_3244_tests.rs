//! #3244 pins — the constructor tear-off's WRITTEN type-argument group and
//! the `unnecessary_type_check` verdict.
//!
//! Upstream reads the operand's static type (`leftNode.typeOrThrow`,
//! best_practices_verifier.dart:1011) and asks
//! `_typeSystem.isSubtypeOf(leftType, rightType)` (:1044). A constructor
//! tear-off is not a generic function value: `C.new` on `class C<T>` is the
//! official generic tear-off `C Function<T>()`, while an INSTANTIATED
//! `C<int>.new` is the non-generic `C<int> Function()` — the written
//! arguments instantiate the class (upstream's `ConstructorReference`
//! resolution, `pkg/analyzer/lib/src/dart/resolver/function_reference_
//! resolver.dart`). The two spellings are different types, so the verdict
//! differs with them: `C.new is C Function<T>()` is constant-true while
//! `C<int>.new is C Function<T>()` is a real runtime question (the oracle
//! stays silent — `C<int> Function()` has no own `<T>` group).
//!
//! dartq's receiver byte walk could not recover the written group here (its
//! entry never keys `call_type_args`, and the #610 `completed_group` latch
//! is a single slot the statement's later heads overwrite), and the
//! bare-receiver fallback then fabricated the generic face — a wrong type
//! that turned the second check into a false-positive row. The pins below
//! lock both directions: the recovered/spelled distinction keeps the bare
//! receiver's generic face and refuses to invent one for a written group.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The `(offset, length, message)` of one code's rows, ascending by offset.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.0);
    rows
}

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from
        + fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// An instantiated tear-off (`C<int>.new`) never answers from the bare
/// receiver's generic face: `C<int> Function()` is not `C Function<T>()`,
/// and reporting the check would be a false positive the oracle does not
/// make.
///
/// Before #3244 the unrecovered `<int>` group fell through to the
/// bare-receiver form, so the verdict read the operand as the generic
/// `C<T> Function<T>()` and reported the check constant-true.
#[test]
fn instantiated_tear_off_does_not_answer_from_the_generic_face() {
    let fixture = "\
class C<T> {
  C();
}
void main() {
  var a = C<int>.new;
  a is C Function<T>();
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "an instantiated tear-off must not be read as the bare generic face: {:?}",
        rows(&report, "unnecessary_type_check")
    );
}

/// The bare receiver keeps its own group: `C.new` on `class C<T>` IS the
/// official generic tear-off `C Function<T>()`, so `C.new is C Function<T>()`
/// stays the constant-true row at the check's own span.
#[test]
fn bare_generic_tear_off_keeps_its_own_type_parameters() {
    let fixture = "\
class C<T> {
  C();
}
void main() {
  var b = C.new;
  b is C Function<T>();
}
";
    let report = report_ops(fixture);
    let check = "b is C Function<T>()";
    let expected = vec![(
        at(fixture, check, 0),
        u32::try_from(check.len()).expect("check span fits u32"),
        "Unnecessary type check; the result is always 'true'.".to_string(),
    )];
    assert_eq!(rows(&report, "unnecessary_type_check"), expected);
}
