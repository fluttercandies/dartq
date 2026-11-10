//! #3248 pins — the invocation faces the #3221 recon left on
//! `diagnose/invocation.rs`:
//!
//! * **W6 — the record-argument appendix.** `argument_type_not_assignable`
//!   between two record types carries the oracle's `additionalInfo`
//!   (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/error_detection_helpers.dart:405-441):
//!   the positional/named arity mismatch and every named field the formal
//!   does not carry, space-joined after the clause. Every dartq emit site
//!   passed `""`, so the rows already existed and only their message
//!   diverged. The pins ride the walks whose rows this file emits itself
//!   (the annotation and enum-constant argument checks): a plain call's or
//!   creation's row is STAGED and emitted by the `engine_core.rs` teardown
//!   (`code.message3(&a0, &a1, "")`, the `StagedKind::Argument` arm), which
//!   is outside this ticket's write scope and still needs the same
//!   `record_arity_note` call.
//! * **W9 — the unindexed `Map`/`List` index formal.**
//!   `checkIndexExpressionIndex` (error_detection_helpers.dart:260-289)
//!   resolves the receiver's `[]`/`[]=` elements; `Map`/`List` carry no
//!   element model in the graph, so `resolved_member` answered nothing and
//!   `Map<int, int> m; m['x'] = 0` stayed silent while the oracle reports
//!   the key mismatch.
//! * **W3 — a closure literal's stub return under a function-typed
//!   formal.** A block-bodied literal captures the `void` return stub
//!   (`inferLocalFunction` re-types the body under the formal's return as
//!   its context), so the stub must re-type before the ask — the written
//!   signature arm already does, the row that resolved against the stub
//!   itself did not, and reported a mismatch the oracle never makes. The
//!   constraint-solve half of the face (`callee_return_ty`'s LUB of the
//!   body's returns) lives in the `assignability.rs` sibling lane.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! and the `assignability.rs` sibling lane are out of this ticket's scope).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The rows of one code, ascending by offset, as `(offset, message)` — the
/// message trimmed of the template's terminal space (the empty appendix
/// keeps it, the filled one does not).
fn rows(report: &FileReport, code: &str) -> Vec<(u32, String)> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.trim_end().to_string()))
        .collect::<Vec<_>>();
    rows.sort_by_key(|(offset, _)| *offset);
    rows
}

/// The offset the annotation and enum-constant argument walks anchor a
/// record literal at: its CLOSING paren. Those two walks report the parsed
/// argument's captured range, which for a record literal is that one
/// character; the call-site walk's whole-expression span is the staged
/// teardown's (see the module note).
fn at_record_end(fixture: &str, needle: &str) -> u32 {
    at(fixture, needle, 0) + u32::try_from(needle.len() - 1).expect("needle fits u32")
}

/// The absolute offset of the `nth` occurrence (0-based) of `needle`:
/// [`report_ops`] prefixes [`CORE_PRELUDE`], so every fixture-local offset
/// shifts by the prelude's length.
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

/// `test_recordType`: a positional record formal against a purely named
/// argument record — the oracle appends the positional count (its
/// `expectedPositionalFields != 0 && actual != expected` arm). The record
/// literal rides an annotation argument, the invocation-side walk that
/// emits its row inside this file's scope; a plain call's row rides the
/// staged teardown instead (see the module note).
///
/// Before #3248 the row carried the bare clause with a trailing space.
#[test]
fn record_argument_positional_arity_is_appended() {
    let fixture = "\
class C {
  const C((int a, int b) r);
}
@C((a: 1, b: 2))
class D {}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at_record_end(fixture, "(a: 1, b: 2)"),
            "The argument type '({int a, int b})' can't be assigned to the parameter type \
             '(int, int)'. Expected 2 positional arguments, but got 0 instead."
                .to_string(),
        )],
        "the positional arity appendix rides the row: {:?}",
        report.diagnostics
    );
}

/// `test_recordType_namedArguments_missing`: the alias-folded formal's named
/// count decides the appendix while the written spelling (`R`) displays.
#[test]
fn record_argument_named_arity_is_appended_through_the_alias() {
    let fixture = "\
typedef R = ({int b, int c});
class C {
  const C(R r);
}
@C((b: 2))
class D {}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at_record_end(fixture, "(b: 2)"),
            "The argument type '({int b})' can't be assigned to the parameter type 'R'. \
             Expected 2 named arguments, but got 1 instead."
                .to_string(),
        )],
        "the named arity appendix rides the row: {:?}",
        report.diagnostics
    );
}

/// `test_recordType_namedArguments`: the counts agree, so only the field the
/// formal does not carry (same name, same type) is appended.
#[test]
fn record_argument_unexpected_named_field_is_appended() {
    let fixture = "\
typedef R = ({int b, int c});
class C {
  const C(R r);
}
@C((bb: 2, c: 3))
class D {}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at_record_end(fixture, "(bb: 2, c: 3)"),
            "The argument type '({int bb, int c})' can't be assigned to the parameter type \
             'R'. Unexpected named argument `bb` with type `int`."
                .to_string(),
        )],
        "the unexpected named field rides the row: {:?}",
        report.diagnostics
    );
}

/// `test_recordType_positionalArguments`: a positional alias formal against a
/// three-field argument record.
#[test]
fn record_argument_positional_arity_through_the_alias() {
    let fixture = "\
typedef R = (int b, int c);
class C {
  const C(R r);
}
@C((3, 2, 1))
class D {}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at_record_end(fixture, "(3, 2, 1)"),
            "The argument type '(int, int, int)' can't be assigned to the parameter type \
             'R'. Expected 2 positional arguments, but got 3 instead."
                .to_string(),
        )],
        "the positional arity appendix rides the aliased formal: {:?}",
        report.diagnostics
    );
}

/// The enum-constant argument walk carries the same appendix (its own
/// emit site, the same reporter): a const record argument against a
/// positional record formal.
#[test]
fn enum_constant_argument_carries_the_appendix() {
    let fixture = "\
enum E {
  a((1, 2, 3));
  const E((int a, int b) r);
  final (int a, int b) r;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at_record_end(fixture, "(1, 2, 3)"),
            "The argument type '(int, int, int)' can't be assigned to the parameter type \
             '(int, int)'. Expected 2 positional arguments, but got 3 instead."
                .to_string(),
        )],
        "the enum constant's record argument carries the appendix: {:?}",
        report.diagnostics
    );
}

/// A non-record pair keeps the bare clause (the appendix is record-only):
/// `int` against `String` appends nothing.
#[test]
fn non_record_argument_keeps_the_bare_clause() {
    let fixture = "\
void f(String s) {}
void main() {
  f(0);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at(fixture, "0", 0),
            "The argument type 'int' can't be assigned to the parameter type 'String'.".to_string(),
        )],
        "no appendix for a non-record pair: {:?}",
        report.diagnostics
    );
}

/// `test_map_indexSet`: `Map<int, int>`'s write element declares `K` as its
/// key, so a `String` index reports at the index span.
///
/// Before #3248 the face was silent — `resolved_member` cannot see the
/// unindexed SDK `Map`.
#[test]
fn map_index_set_checks_the_key_against_the_written_argument() {
    let fixture = "\
void main(Map<int, int> m) {
  m['x'] = 0;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at(fixture, "'x'", 0),
            "The argument type 'String' can't be assigned to the parameter type 'int'.".to_string(),
        )],
        "the write element's key formal decides the index argument: {:?}",
        report.diagnostics
    );
}

/// `test_map_indexSet_ifNull`: the compound write form checks the same `[]=`
/// element; the read `[]` element's key accepts every argument.
#[test]
fn map_index_if_null_set_checks_the_key() {
    let fixture = "\
void main(Map<int, int> m) {
  m['x'] ??= 0;
}
";
    let report = report_ops(fixture);
    let atna = rows(&report, "argument_type_not_assignable");
    assert_eq!(
        atna.first(),
        Some(&(
            at(fixture, "'x'", 0),
            "The argument type 'String' can't be assigned to the parameter type 'int'.".to_string(),
        )),
        "the compound write form checks the key formal too: {:?}",
        report.diagnostics
    );
}

/// A matching key stays silent — the staged face reports only a genuine
/// mismatch (the under-report guard on the new core formal).
#[test]
fn map_index_set_with_the_written_key_stays_silent() {
    let fixture = "\
void main(Map<int, int> m) {
  m[0] = 0;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        Vec::new(),
        "an `int` key against `Map<int, int>` is silent: {:?}",
        report.diagnostics
    );
}

/// `test_unsafeBlockClosureInference_functionCall_explicitTypeParam`: a
/// block-bodied closure's captured return is the `void` stub, and the
/// formal's `T` is the downward context the body re-infers under — the stub
/// re-types and the row stays silent (`inferLocalFunction`'s cap).
///
/// Before #3248 the resolved stub row was asked as written and reported
/// `void Function()` against `T Function()`.
#[test]
fn block_closure_return_retypes_under_a_type_parameter_context() {
    let fixture = "\
List<T> f<T>(T g()) => <T>[g()];
var v = f<int>(() { return 1; });
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        Vec::new(),
        "the closure's return re-types to the formal's `T`: {:?}",
        report.diagnostics
    );
}

/// The re-typing is the CLOSURE literal's alone: a tear-off carries its real
/// written signature, so `void Function()` against `int Function()` still
/// reports (the guard on the #3248 relaxation).
#[test]
fn void_tearoff_against_a_concrete_function_formal_still_reports() {
    let fixture = "\
void g() {}
void take(int Function() f) {}
void main() {
  take(g);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable"),
        vec![(
            at(fixture, "take(g)", 0) + 5,
            "The argument type 'void Function()' can't be assigned to the parameter type \
             'int Function()'."
                .to_string(),
        )],
        "a tear-off keeps its written return: {:?}",
        report.diagnostics
    );
}
