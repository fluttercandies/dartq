//! #3359 pins — the return-type latch of executables, the augment field
//! group's own initialization obligation, and the listener riders (the
//! `void` member-send receiver anchor, the brace literal's side, #3393).
//!
//! * Return latch: `begin_method` fires before the return annotation
//!   parses, so an operator (it has no name-identifier event) latches with
//!   the armed slot cleared, and the annotation backfills through
//!   `Engine::arm_return_span` — which only accepts a type written ahead of
//!   the executable's name. A type completed inside the body (after a
//!   nested closure closed its frame) or a stale `extends`/`on` clause type
//!   never becomes the host's return: an unannotated executable returns
//!   `dynamic`, and the completion/return checks stay silent on it.
//! * Augment field group: upstream checks every `FieldDeclaration` node,
//!   augmentations included (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/error_verifier.dart
//!   `_checkForNotInitializedFieldDeclaration`), exempt only by the node's
//!   own `abstract`/`external` keyword.
//! * `void` receiver: a member send reports `use_of_void_result` at the
//!   whole receiver node (`useOfVoidResult.at(receiver)`).
//! * Brace literal side (`typed_literal_resolver.dart`): a null-aware key
//!   (`?k: v`) is a map entry, the `?` of `??`/`?.` is no conditional, and
//!   a literal of spreads and control-flow elements takes its side from
//!   the context, then from its leaves.
//!
//! Anchors locate themselves with `find` over the prelude-prefixed source.
//! Registered through `diagnose/tests.rs`'s `#[path]` tail.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// Diagnoses `fixture` behind the core prelude; the report's offsets index
/// the returned full source.
fn run(fixture: &str) -> (FileReport, String) {
    (report_ops(fixture), format!("{CORE_PRELUDE}{fixture}"))
}

/// The rows of `code` as their exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| full[d.range.offset as usize..d.range.end() as usize].to_string())
        .collect()
}

/// The completion/return family as `code text` rows, source order.
fn return_rows(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "body_might_complete_normally"
                    | "body_might_complete_normally_nullable"
                    | "return_of_invalid_type"
                    | "return_without_value"
            )
        })
        .map(|d| {
            let text = &full[d.range.offset as usize..d.range.end() as usize];
            format!("{} {text}", d.code.name)
        })
        .collect()
}

#[track_caller]
fn assert_no_return_rows(fixtures: &[&str]) {
    for fixture in fixtures {
        let (report, full) = run(fixture);
        let rows = return_rows(&report, &full);
        assert!(rows.is_empty(), "{fixture:?}: {rows:?}");
    }
}

// —— Return latch ————————————————————————————————————————————————————

/// An unannotated function returns `dynamic`: neither a bare nor a valued
/// `return` reports, and a typed declaration ahead of it lends no type.
#[test]
fn unannotated_function_returns_dynamic() {
    assert_no_return_rows(&[
        "f() { return; }\n",
        "f() { return 1; }\n",
        "int x = 0;\nf() { return; }\n",
        "int x = 0;\ng() {}\n",
    ]);
}

/// A type completed in the body after a nested closure closed its frame
/// is no return annotation.
#[test]
fn body_type_after_a_closure_is_no_return_type() {
    assert_no_return_rows(&[
        "f() { var g = () {}; int x = 0; }\n",
        "void h(Object o) {}\nf() { h(() {}); int x = 0; }\n",
    ]);
}

/// A written return still reports a body that falls off the end, at the
/// declaring name.
#[test]
fn annotated_function_reports_at_its_name() {
    let (report, full) = run("int f() {}\n");
    assert_eq!(
        return_rows(&report, &full),
        vec!["body_might_complete_normally f"]
    );
}

/// A generic return annotation keeps its arguments (the backfill joins
/// the completed group).
#[test]
fn generic_return_annotation_keeps_its_arguments() {
    let (report, _) = run("List<int> f() {}\n");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "body_might_complete_normally")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("'List<int>'"), "{messages:?}");
}

/// An unannotated operator returns `dynamic` — never the type the
/// enclosing `extends`/`on` clause armed last.
#[test]
fn unannotated_operator_keeps_dynamic() {
    assert_no_return_rows(&[
        "class B {}\nclass C extends B { operator +(other) { return 1; } }\n",
        "class B {}\nclass C extends B { operator -(other) {} }\n",
        "extension E on String { operator -(other) {} }\n",
    ]);
}

/// A written operator return still reports, at the operator token.
#[test]
fn annotated_operator_reports_at_the_operator() {
    let (report, full) = run("class C { int operator +(C o) {} }\n");
    assert_eq!(
        return_rows(&report, &full),
        vec!["body_might_complete_normally +"]
    );
}

// —— Augment field group ——————————————————————————————————————————————

/// An `augment` field group owes its own initializer: a `final` field with
/// no initializer in a constructor-less class reports once, at its name.
#[test]
fn augment_final_field_reports_final_not_initialized() {
    let (report, full) = run("class C {}\naugment class C {\n  final int x;\n}\n");
    assert_eq!(spans_of(&report, &full, "final_not_initialized"), vec!["x"]);
}

/// The group's own `abstract`/`external` keyword exempts it, and an
/// extension type's instance field owns no initializer row.
#[test]
fn augment_field_exemptions_stay_silent() {
    for fixture in [
        "class C {}\naugment class C {\n  abstract final int x;\n}\n",
        "class C {}\naugment class C {\n  external final int x;\n}\n",
        "extension type ET(int v) {}\naugment extension type ET {\n  final int y;\n}\n",
    ] {
        let (report, full) = run(fixture);
        let rows = spans_of(&report, &full, "final_not_initialized");
        assert!(rows.is_empty(), "{fixture:?}: {rows:?}");
    }
}

// —— `void` member-send receiver —————————————————————————————————————

/// `v().toString()` reports once, spanning the whole receiver send.
#[test]
fn void_send_receiver_anchors_at_the_whole_receiver() {
    let (report, full) = run("void v() {}\nvoid f() { v().toString(); }\n");
    assert_eq!(spans_of(&report, &full, "use_of_void_result"), vec!["v()"]);
}

// —— Brace literal side (#3393) ———————————————————————————————————————

const BOTH: &str = "ambiguous_set_or_map_literal_both";

/// A null-aware key is a map entry: beside an expression element (or an
/// iterable spread) the literal is both.
#[test]
fn null_aware_key_is_a_map_entry() {
    for (fixture, literal) in [
        (
            "void f(String? k, String? v) { var c = {?v, ?k: 'x'}; }\n",
            "{?v, ?k: 'x'}",
        ),
        (
            "void f(String? k, String? v) { var c = {?k: ?v, 'e'}; }\n",
            "{?k: ?v, 'e'}",
        ),
        (
            "void f(String? k, List<int> l) { var c = {?k: 'x', ...l}; }\n",
            "{?k: 'x', ...l}",
        ),
    ] {
        let (report, full) = run(fixture);
        assert_eq!(spans_of(&report, &full, BOTH), vec![literal], "{fixture:?}");
    }
}

/// Entries only — null-aware, if-null, and null-aware-access keys
/// included — are a map; a conditional element beside an entry is both.
#[test]
fn entry_keys_are_no_conditionals() {
    for fixture in [
        "void f(String? k) { var c = {?k: 'a', 'b': 'c'}; }\n",
        "void f(int? a) { var c = {a ?? 0: 1, 2: 3}; }\n",
        "class A { int? get id => null; }\nvoid f(A? a) { var c = {a?.id: 1, 2: 3}; }\n",
    ] {
        let (report, full) = run(fixture);
        let rows = spans_of(&report, &full, BOTH);
        assert!(rows.is_empty(), "{fixture:?}: {rows:?}");
    }
    let (report, full) = run("void f(bool b) { var c = {b ? 1 : 2, 3: 4}; }\n");
    assert_eq!(spans_of(&report, &full, BOTH), vec!["{b ? 1 : 2, 3: 4}"]);
}

/// A literal of spreads and control-flow elements under a set context is
/// a set: the return adopts the context instead of a `Map` default.
#[test]
fn control_flow_literal_adopts_its_set_context() {
    assert_no_return_rows(&[
        "Set<int> f(List<int> xs) => {...xs};\n",
        "Set<int> f(List<int> xs) => {for (var x in xs) x};\n",
        "Map<int, int> f(List<int> xs) => {for (var x in xs) x: x};\n",
    ]);
}
