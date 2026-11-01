//! #3504 pins — `return_of_invalid_type` inside extension members reads
//! the member's own declared return annotation, never the extension's
//! `on` type.
//!
//! Upstream `_checkForReturnOfInvalidType` resolves the expected type
//! from `enclosingExecutable.returnType` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart): the executable
//! is the extension's own member, so the `on` clause type only arms the
//! container face — it must not leak into a member's return verdict.
//! An operator member latches its `ret` before its annotation parses,
//! so the armed `on` type was the stale slot's content; the latch now
//! drops it and `Engine::arm_return_span` backfills only a completion
//! that starts before the declaring name.
//!
//! Anchors locate themselves with `find` over the prelude-prefixed
//! source. Registered through `diagnose/tests.rs`'s `#[path]` tail.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// Diagnoses `fixture` behind the core prelude; the report's offsets
/// index the returned full source.
fn run(fixture: &str) -> (FileReport, String) {
    (report_ops(fixture), format!("{CORE_PRELUDE}{fixture}"))
}

/// The `return_of_invalid_type` rows as `(anchored text, message)` pairs,
/// source order.
fn invalid_type_rows(report: &FileReport, full: &str) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| {
            (
                full[d.range.offset as usize..d.range.end() as usize].to_string(),
                d.message.to_string(),
            )
        })
        .collect()
}

#[track_caller]
fn assert_silent(fixtures: &[&str]) {
    for fixture in fixtures {
        let (report, full) = run(fixture);
        let rows = invalid_type_rows(&report, &full);
        assert!(rows.is_empty(), "{fixture:?}: {rows:?}");
    }
}

// —— Silent faces (the recon repro) ——————————————————————————————————

/// A matching member return reports nothing: the extension's `on` type
/// is no return annotation. `Function` is the repro that first split —
/// the `on` clause armed `Function`, the member's `bool` was judged
/// against it, and `true` read as a bogus `Function` return.
#[test]
fn matching_extension_member_returns_stay_silent() {
    assert_silent(&[
        "extension E on Function {\n  bool operator <(Object o) => true;\n}\n",
        "class C {}\nextension E on C {\n  bool operator <(Object o) => true;\n}\n",
        "extension E on Function {\n  bool m() => true;\n}\n",
        "extension E on int {\n  int m() => 1;\n}\n",
    ]);
}

/// An unannotated extension member returns `dynamic`: the `on` type is
/// not its return type, so even a mismatched body reports nothing.
#[test]
fn unannotated_extension_member_keeps_dynamic() {
    assert_silent(&[
        "class C {}\nextension E on C {\n  operator <(Object o) => \"s\";\n}\n",
        "extension E on int {\n  m() => \"s\";\n}\n",
    ]);
}

// —— Reported faces (the fix must not silence the code wholesale) ————

/// A genuine mismatch inside an extension member reports the member's
/// own return type — `on int`/`on C`/`on String` never stand in as the
/// expected type.
#[test]
fn mismatch_inside_an_extension_member_reports_the_member_return() {
    for (fixture, expected) in [
        (
            "extension E on int {\n  String m() => 1;\n}\n",
            "'String'",
        ),
        (
            "class C {}\nextension E on C {\n  String m() => 1;\n}\n",
            "'String'",
        ),
        (
            "extension E on String {\n  int m() => \"s\";\n}\n",
            "'int'",
        ),
    ] {
        let (report, _) = run(fixture);
        let messages: Vec<String> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "return_of_invalid_type")
            .map(|d| d.message.to_string())
            .collect();
        assert_eq!(messages.len(), 1, "{fixture:?}: {messages:?}");
        assert!(
            messages[0].contains(expected),
            "{fixture:?}: {messages:?}"
        );
    }
}

/// An operator member reports its own annotation — `on bool` does not
/// become `bool`'s return, and `on C` does not become `C`'s.
#[test]
fn extension_operator_reports_its_own_annotation() {
    let (report, _) = run(
        "extension E on bool {\n  bool operator <(Object o) => \"s\";\n}\n",
    );
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("'bool'"), "{messages:?}");

    let (report, full) = run(
        "class C {}\nextension E on C {\n  int operator <(Object o) => \"s\";\n}\n",
    );
    assert_eq!(
        invalid_type_rows(&report, &full),
        vec![(
            "\"s\"".to_string(),
            "A value of type 'String' can't be returned from the method '<' because it has a return type of 'int'.".to_string()
        )]
    );
}

/// A sibling member's annotation does not bleed across: `int m()` after
/// `D operator <` keeps `D` as the operator's expected type.
#[test]
fn sibling_member_annotations_do_not_bleed() {
    let (report, full) = run(
        "class D {}\nclass C {}\nextension E on C {\n  D operator <(Object o) => \"s\";\n  int m() => 1;\n}\n",
    );
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("'D'"), "{messages:?}");
    let _ = full;
}

/// An `extension type` member follows the same rule: the representation
/// type is no return annotation.
#[test]
fn extension_type_member_reports_its_own_return() {
    let (report, _) =
        run("extension type ET(int v) {\n  String m() => 1;\n}\n");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("'String'"), "{messages:?}");
}

// —— Class control ——————————————————————————————————————————————————

/// A plain class member behaves identically: its own annotation is the
/// expected type, the row anchors at the returned expression.
#[test]
fn class_member_control() {
    let (report, full) = run("class C {\n  int m() => \"s\";\n}\n");
    assert_eq!(
        invalid_type_rows(&report, &full),
        vec![(
            "\"s\"".to_string(),
            "A value of type 'String' can't be returned from the method 'm' because it has a return type of 'int'.".to_string()
        )]
    );
}
