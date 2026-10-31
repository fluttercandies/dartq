//! #3276 pins — the record receiver's FIELD-WRITE face.
//!
//! Records are immutable upstream: `type_property_resolver`'s record arm
//! resolves a receiver's `fieldByName(name)` for the READ face only and
//! returns `ResolutionResult(recordField: field, needsGetterError: false)`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart:218-225).
//! No setter element is ever synthesized for an `$n` or named field, so a
//! write to an EXISTING field resolves nothing on the write side and the
//! resolver's setter face reports `undefined_setter` on the record display
//! (`property_element_resolver.dart:619-625` — the `readResult` recovery is
//! a record field, which takes the `AssignmentVerifier` tail's
//! `undefined_setter` branch, `assignment_verifier.dart:90-99`).
//!
//! Two halves are pinned:
//!
//! * the WRITE verdict — a named or positional field write, on a record
//!   literal's own type and on an annotated record parameter alike, reports
//!   the oracle's `undefined_setter` row and nothing else, while the same
//!   name read stays resolved;
//! * the COMPOUND read face — `r.foo += 1` reports the getter miss when a
//!   field is absent and an extension setter alone resolves the write
//!   (the two flags are independent upstream, so the setter's resolution
//!   never clears the getter's verdict).
//!
//! The negative twin guards the face: a non-record receiver's field write
//! and a getter/setter-paired extension stay silent.
//!
//! Out of scope, recorded: the record receiver's DISPLAY still renders the
//! `typedef` expansion (`(int, String, {int x})`) where the oracle names the
//! alias (`R`) — the same alias-display gap every typedef receiver shows
//! (class, function and record alike), which needs alias identity on the
//! type model rather than a resolution-path change. The extension
//! getter-ONLY recovery (`assignment_to_final_no_setter`, "There isn't a
//! setter named 'foo' in class 'E'.") is likewise the assignment-to-*
//! family's unimplemented face, not this path's.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
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

/// The messages of one code's rows, in report order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// Every emitted code name, ascending, so a pin can assert silence whole.
fn code_names(report: &FileReport) -> Vec<&'static str> {
    let mut names = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

/// The headline residual: a write to an EXISTING named record field reports
/// `undefined_setter` on the record display — the field's getter resolves
/// the read face only, and records synthesize no setter.
#[test]
fn existing_named_field_write_reports_undefined_setter() {
    let fixture = "void f() {\n\
         \x20 var r = (bar: 1);\n\
         \x20 r.bar = 3;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "bar = 3", 0)]);
    assert_eq!(
        messages(&report, "undefined_setter"),
        vec!["The setter 'bar' isn't defined for the type '({int bar})'.".to_string()]
    );
    // No getter row: the same name still reads.
    assert_eq!(offsets(&report, "undefined_getter"), Vec::<u32>::new());
}

/// The corpus' own shape (`AssignmentExpressionResolutionTest`'s
/// `test_propertyAccess_recordTypeField_*` cases, co19 `Records/members_A02_t06`):
/// an ANNOTATED record parameter, named field sorted into the display.
#[test]
fn annotated_parameter_named_field_write_reports_undefined_setter() {
    let fixture = "void f(({int foo, String bar}) r) {\n\
         \x20 r.foo = 0;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "foo = 0", 0)]);
    assert_eq!(
        messages(&report, "undefined_setter"),
        vec![
            "The setter 'foo' isn't defined for the type '({String bar, int foo})'.".to_string()
        ]
    );
}

/// The positional twin: `$0` is a getter like any named field, so the write
/// misses identically (`Records/members_A03_t06`'s `$1` rows).
#[test]
fn positional_field_write_reports_undefined_setter() {
    let fixture = "void f() {\n\
         \x20 var p = (1, 'x');\n\
         \x20 p.$0 = 3;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "$0 = 3", 0)]);
    assert_eq!(
        messages(&report, "undefined_setter"),
        vec!["The setter '$0' isn't defined for the type '(int, String)'.".to_string()]
    );
}

/// Beyond the last positional slot the same row fires: the miss face does
/// not depend on the field existing (`$9` over a 2-field record).
#[test]
fn out_of_range_positional_write_reports_the_same_row() {
    let fixture = "void f() {\n\
         \x20 var p = (1, 'x');\n\
         \x20 p.$9 = 3;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "$9 = 3", 0)]);
    assert_eq!(
        messages(&report, "undefined_setter"),
        vec!["The setter '$9' isn't defined for the type '(int, String)'.".to_string()]
    );
}

/// The non-existent-field twin reports the identical row: a declared field
/// and a missing one are indistinguishable on the write face (both resolve
/// no setter), so one gate answers both.
#[test]
fn missing_field_write_reports_the_same_row() {
    let fixture = "void f() {\n\
         \x20 var r = (bar: 1);\n\
         \x20 r.baz = 3;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "baz = 3", 0)]);
    assert_eq!(
        messages(&report, "undefined_setter"),
        vec!["The setter 'baz' isn't defined for the type '({int bar})'.".to_string()]
    );
}

/// The read face is untouched: reading the same field resolves silently.
#[test]
fn existing_field_read_stays_silent() {
    let fixture = "int f() {\n\
         \x20 var r = (bar: 1);\n\
         \x20 return r.bar;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(code_names(&report), Vec::<&'static str>::new());
}

/// A compound write over an existing field reports the setter alone — the
/// getter resolved through the field, so only the write face misses.
#[test]
fn compound_write_over_existing_field_reports_the_setter_alone() {
    let fixture = "void f() {\n\
         \x20 var r = (bar: 1);\n\
         \x20 r.bar += 1;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_setter"), vec![at(fixture, "bar += 1", 0)]);
    assert_eq!(offsets(&report, "undefined_getter"), Vec::<u32>::new());
}

/// The compound read face's own verdict (this ticket's gate): the record
/// lacks the field and an extension supplies only `set foo`, so the write
/// resolves while the read misses — `undefined_getter` on the record
/// display, no setter row.
#[test]
fn compound_write_with_extension_setter_only_reports_the_getter() {
    let fixture = "extension E on ({int bar}) {\n\
         \x20 set foo(int _) {}\n\
         }\n\
         void f(({int bar}) r) {\n\
         \x20 r.foo += 0;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(offsets(&report, "undefined_getter"), vec![at(fixture, "foo += 0", 0)]);
    assert_eq!(
        messages(&report, "undefined_getter"),
        vec!["The getter 'foo' isn't defined for the type '({int bar})'.".to_string()]
    );
    assert_eq!(offsets(&report, "undefined_setter"), Vec::<u32>::new());
}

/// The extension setter alone resolves a plain write: silent (the extension
/// application owns the credit, no row).
#[test]
fn plain_write_with_extension_setter_stays_silent() {
    let fixture = "extension E on ({int bar}) {\n\
         \x20 set foo(int _) {}\n\
         }\n\
         void f(({int bar}) r) {\n\
         \x20 r.foo = 0;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(code_names(&report), Vec::<&'static str>::new());
}

/// A getter/setter pair resolves both faces of a compound write: silent.
#[test]
fn compound_write_with_extension_pair_stays_silent() {
    let fixture = "extension E on ({int bar}) {\n\
         \x20 int get foo => 0;\n\
         \x20 set foo(int _) {}\n\
         }\n\
         void f(({int bar}) r) {\n\
         \x20 r.foo += 0;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(code_names(&report), Vec::<&'static str>::new());
}

/// The non-record negative: a class field's write resolves its implicit
/// setter, so the record gate never fires.
#[test]
fn class_field_write_stays_silent() {
    let fixture = "class C {\n\
         \x20 var bar;\n\
         }\n\
         void f(C c) {\n\
         \x20 c.bar = 3;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(code_names(&report), Vec::<&'static str>::new());
}
