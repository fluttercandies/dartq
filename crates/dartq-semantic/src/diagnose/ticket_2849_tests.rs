//! The #2849 annotation-fold seam pins: a formal whose written
//! annotation is a parameterized function-RHS alias use (`F<int>`)
//! joins the written-signature stash with the INSTANTIATED signature —
//! the element model binds the annotation's resolved type, the written
//! arguments substituted into the RHS variables' bounds (upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/resolver/
//! named_type_resolver.dart `_instantiateElement` →
//! `TypeAliasElementImpl.instantiateImpl`). The assignment pair's
//! equality rule (#2834) then decides over real populated params, and
//! the byte-exact message spells the resolved instantiations both ways.

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

/// The `invalid_assignment` rows of `report`, messages only.
fn assignment_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect()
}

/// All diagnostics of `report`, code names only.
fn codes(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_string())
        .collect()
}

/// The substituted POSITIONAL rides the instantiated signature: the
/// alias RHS formal `X cb` reads `int` on the target side against the
/// written `num cb` source — the message spells both resolved shapes.
#[test]
fn substituted_positional_rides_the_instantiated_stash() {
    let report = report_ops(
        "typedef H<X> = void Function<Y extends X>(X cb);\n\
         \x20 void f(H<int> a, void Function<Y extends num>(num cb) b) {\n\
         \x20   a = b;\n\
         \x20 }",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'void Function<Y extends num>(num)' can't be assigned to a variable of type 'void Function<Y extends int>(int)'.".to_string(),
        ],
        "rows: {:#?}",
        rows(&report)
    );
}

/// A BARE alias use (`F` — no written arguments) binds no stash (the
/// claim gates on written arguments): the written-shape face answers
/// the pair with the parameter count (1 against 0) — the row the
/// oracle also reports (the bare form instantiates to bounds, `dynamic`
/// against `num`, the equality fails); the resolved `void Function<Y
/// extends dynamic>()` spelling is the raw bridge's business (#2838).
/// The source side renders its real bounds through the stash (#2849's
/// display upgrade).
#[test]
fn bare_alias_use_reports_through_the_written_shape_face() {
    let report = report_ops(
        "typedef F<X> = void Function<Y extends X>();\n\
         \x20 void f(F a, void Function<Y extends num>() b) {\n\
         \x20   a = b;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 1, "rows: {:#?}", rows(&report));
    assert!(
        messages[0].contains("A value of type 'void Function<Y extends num>()' can't be assigned"),
        "{messages:#?}"
    );
}

/// The new stash entry serves the callable face unchanged: an
/// alias-typed formal still invokes (void return, no arguments) with
/// no rows.
#[test]
fn alias_typed_formal_stays_callable() {
    let report = report_ops(
        "typedef F<X> = void Function<Y extends X>();\n\
         \x20 void f(F<int> a) {\n\
         \x20   a();\n\
         \x20 }",
    );
    assert!(codes(&report).is_empty(), "rows: {:#?}", rows(&report));
}
