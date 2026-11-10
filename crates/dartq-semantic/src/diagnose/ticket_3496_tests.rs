//! #3496 pins — a function-typed receiver reaches the extension pool.
//!
//! # The defect
//!
//! `check_shape_receiver`'s FUNCTION arm consulted only the shape-vs-shape
//! extension ask (`record_extension_face` → `function_extension_face`, i.e.
//! faces whose `on` type spells a function type). A receiver whose static
//! type is a function SHAPE carries no container head, so the name-level
//! `extension_application` consult the spelled `Function` interface already
//! runs (`check_core_type_member`) never happened: `extension StaticType<T>
//! on T` applied to the same value through a static tear-off (typed as a
//! `Function` HEAD in this model) and did not through a function-typed
//! value, and the miss tail reported `undefined_method ... 'Function'`
//! where the oracle is silent (the census's
//! `type_annotation_inheritance_A01_t0{6,7}` rows).
//!
//! # The contract
//!
//! `_lookupExtension` runs against the receiver type itself (upstream Dart
//! SDK source,
//! pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart:229-232),
//! and a function type subtypes the only named heads such a face can
//! spell — `Function` and `Object` — plus a bare own-parameter `on T` whose
//! bound admits it. The consult therefore asks the access through
//! `extension_application` with the receiver's `Function` head, exactly the
//! pair `check_core_type_member` feeds for the spelled `Function`
//! interface; the shape faces keep their own ask beside it.
//!
//! Message text is the oracle's `undefined_method` template
//! (`The method '{0}' isn't defined for the type '{1}'.`).

use super::test_rig::{code_names, report_ops, rows};

/// The `undefined_method` messages of `report`, in source order.
fn method_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.message.to_string())
        .collect()
}

/// The census shape: a bare own-parameter extension declares the member, so
/// the function-typed receiver resolves through the pool — silent.
#[test]
fn own_parameter_extension_covers_a_function_receiver() {
    let report = report_ops(
        "extension FnProbe<T> on T {\n\
         \x20 int probe() => 1;\n\
         }\n\
         void f(void Function(int) g) {\n\
         \x20 g.probe();\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(&report)
    );
}

/// The same receiver against a `Function`-headed face: the name-level
/// consult admits it (a function type IS a `Function`).
#[test]
fn function_headed_extension_covers_a_function_receiver() {
    let report = report_ops(
        "extension FnHead on Function {\n\
         \x20 int head() => 1;\n\
         }\n\
         void f(void Function(int) g) {\n\
         \x20 g.head();\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(&report)
    );
}

/// An `Object`-headed face covers every receiver, a function shape
/// included (the root head the shape ask never spells).
#[test]
fn object_headed_extension_covers_a_function_receiver() {
    let report = report_ops(
        "extension ObjProbe on Object {\n\
         \x20 int probe() => 1;\n\
         }\n\
         void f(void Function(int) g) {\n\
         \x20 g.probe();\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "expected no rows, got {:#?}",
        rows(&report)
    );
}

/// The FIRING twin: the extension declares a DIFFERENT member, so the miss
/// keeps its row — `undefined_method` naming the function face, exactly the
/// oracle's text for a function receiver with no applicable declaration.
#[test]
fn a_member_no_face_declares_still_reports_the_function_face() {
    assert_eq!(
        method_rows(
            "extension FnProbe<T> on T {\n\
             \x20 int probe() => 1;\n\
             }\n\
             void f(void Function(int) g) {\n\
             \x20 g.gone();\n\
             }\n",
        ),
        ["The method 'gone' isn't defined for the type 'Function'."],
    );
}

/// A BOUNDED own-parameter face does not admit a function receiver
/// (`void Function(int)` is not a `num`), so the miss reports: the consult
/// must never turn every `on T` into a blanket match.
#[test]
fn a_bound_the_function_shape_misses_keeps_the_report() {
    let report = report_ops(
        "extension NumProbe<T extends num> on T {\n\
         \x20 int probe() => 1;\n\
         }\n\
         void f(void Function(int) g) {\n\
         \x20 g.probe();\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_method"],
        "{:#?}",
        rows(&report)
    );
}

/// `call` keeps resolving the function signature itself (the
/// `callFunctionType` arm), never the extension face — the pre-existing
/// behaviour the consult must not disturb.
#[test]
fn call_still_resolves_the_function_signature() {
    let report = report_ops(
        "extension FnProbe<T> on T {\n\
         \x20 int call(int x) => x;\n\
         }\n\
         void f(void Function(int) g) {\n\
         \x20 g.call(1);\n\
         }\n",
    );
    assert!(
        !rows(&report)
            .iter()
            .any(|row| row.starts_with("undefined_method")),
        "call resolves the signature: {:#?}",
        rows(&report)
    );
}
