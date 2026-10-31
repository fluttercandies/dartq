//! #3186 — the type-literal receiver face runs before the invalid-receiver
//! `new` face.
//!
//! A receiver head that SPELLS a type — `dynamic`, `Never`, or an
//! ungrounded type alias — denotes the `Type` INSTANCE even when the
//! receiver face never resolved; the member verdict is the `Type` one, and
//! the alias INVOKE twin belongs to the invocation walk's
//! `report_alias_selector_face` (claimed by
//! `check_type_literal_receiver`, quiet here). The `Receiver::Unknown` +
//! `new` arm used to answer first, pairing the correct row with its own
//! `<unknown>` display. A head that is NO type literal (`Undefined.new()`)
//! keeps that face.
//!
//! Verified verbatim against oracle `dart analyze` 3.13.7:
//! - `typedef Fn = void Function(); Fn.new();` → ONE row,
//!   "The method 'new' isn't defined for the type 'Type'."
//! - `var a = Fn.new;` → "The getter 'new' isn't defined for the type
//!   'Type'." (never the `InvalidType` the failed lookup left)
//! - `Never.new();` → "The method 'new' isn't defined for the type 'Type'."
//! - `Nope.new();` → "The method 'new' isn't defined for the type
//!   '<unknown>'." (plus `undefined_identifier` on the head)

use super::test_rig::report_ops;
use crate::FileReport;

/// The messages of one code, in report order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// `Alias.new()`: exactly ONE row naming `Type` — the duplicate
/// `<unknown>` twin is gone.
#[test]
fn alias_new_reports_one_type_row() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void main() { Fn.new(); }\n",
    );
    let methods = messages(&report, "undefined_method");
    assert_eq!(
        methods,
        ["The method 'new' isn't defined for the type 'Type'."],
        "{methods:?}"
    );
}

/// The alias's READ face takes the same claim: the invalid receiver's
/// `InvalidType` display never surfaces for a type-literal head.
#[test]
fn alias_new_read_names_type() {
    let report = report_ops(
        "typedef Fn = void Function();\n\
         void main() { var a = Fn.new; print(a); }\n",
    );
    let getters = messages(&report, "undefined_getter");
    assert_eq!(
        getters,
        ["The getter 'new' isn't defined for the type 'Type'."],
        "{getters:?}"
    );
}

/// The `Never` keyword head is a type literal too — the row names `Type`
/// (the invalid-receiver arm used to swallow it whole).
#[test]
fn never_new_names_type() {
    let report = report_ops("void main() { Never.new(); }\n");
    let methods = messages(&report, "undefined_method");
    assert_eq!(
        methods,
        ["The method 'new' isn't defined for the type 'Type'."],
        "{methods:?}"
    );
}

/// A head that spells no type keeps the invalid-receiver face: the
/// negative case never takes the type-literal claim.
#[test]
fn unknown_head_new_keeps_unknown_face() {
    let report = report_ops("void main() { Nope.new(); }\n");
    let methods = messages(&report, "undefined_method");
    assert_eq!(
        methods,
        ["The method 'new' isn't defined for the type '<unknown>'."],
        "{methods:?}"
    );
    let names = messages(&report, "undefined_identifier");
    assert_eq!(names, ["Undefined name 'Nope'."], "{names:?}");
}
