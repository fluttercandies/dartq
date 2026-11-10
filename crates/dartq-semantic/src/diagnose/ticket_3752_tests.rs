//! #3752 — imported type-alias receiver heads resolve through the aliased
//! interface.
//!
//! A type-alias head in a member position (`Pub.sm()`) resolves upstream to
//! the interface its right-hand side names, never to the `Type` literal
//! (`_resolveTargetInterfaceElement`, pkg/analyzer/lib/src/dart/resolver/
//! property_element_resolver.dart:507-517; `_resolveReceiverTypeLiteral`,
//! method_invocation_resolver.dart:168-179). The same-file arm reads the
//! alias capture of the diagnosing unit; an IMPORTED alias
//! (`typedef Pub = _Private;` declared in another library, whose target is
//! private to that library) had no capture and fell through to the `Type`
//! face, reporting "…isn't defined for the type 'Type'." where the oracle
//! names the aliased class.
//!
//! Oracle provenance (probed 3.13.7 against `dart analyze --format machine`
//! on the exact shapes below): `Pub.sm()` resolves, `Pub.gone()` reports
//! `The method 'gone' isn't defined for the type '_Private'.`, the generic
//! head twin names `_G`, and both static hits stay silent.

use super::test_rig::{RigImport, assert_silent, report_ops_imports, rows};

/// The private-target library every pin imports: one plain class, one
/// generic class, and the aliases that reach them.
const PRIVATE_LIBRARY: &str = "class _Private {\n\
                                   static void sm() {}\n\
                                   void im() {}\n\
                                 }\n\
                                 typedef Pub = _Private;\n\
                                 class _G<T> {\n\
                                   static void gsm() {}\n\
                                   T? value;\n\
                                 }\n\
                                 typedef PubG<T> = _G<T>;\n\
                                 typedef PubGInt = _G<int>;\n";

fn library_import() -> RigImport<'static> {
    RigImport {
        uri: "package:t/private_name_library.dart",
        prefix: None,
        source: PRIVATE_LIBRARY,
    }
}

/// Every `undefined_method` row the report carries, message-only.
fn undefined_methods(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.message.to_string())
        .collect()
}

#[test]
fn imported_alias_static_member_resolves() {
    let report = report_ops_imports(
        "void main() {\n  Pub.sm();\n  PubG.gsm();\n  PubGInt.gsm();\n}\n",
        &[library_import()],
    );
    assert_silent(&report);
}

#[test]
fn imported_alias_static_miss_names_the_aliased_class() {
    let report = report_ops_imports("void main() {\n  Pub.gone();\n}\n", &[library_import()]);
    assert_eq!(
        undefined_methods(&report),
        ["The method 'gone' isn't defined for the type '_Private'."],
        "rows: {:#?}",
        rows(&report)
    );
}

#[test]
fn imported_generic_alias_static_miss_names_the_aliased_class() {
    let report = report_ops_imports(
        "void main() {\n  PubG.goneG();\n  PubGInt.goneI();\n}\n",
        &[library_import()],
    );
    assert_eq!(
        undefined_methods(&report),
        [
            "The method 'goneG' isn't defined for the type '_G'.",
            "The method 'goneI' isn't defined for the type '_G'.",
        ],
        "rows: {:#?}",
        rows(&report)
    );
}

#[test]
fn imported_alias_static_getter_miss_names_the_aliased_class() {
    let report = report_ops_imports("void main() {\n  Pub.goneGetter;\n}\n", &[library_import()]);
    let getters: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        getters,
        ["The getter 'goneGetter' isn't defined for the type '_Private'."],
        "rows: {:#?}",
        rows(&report)
    );
}
