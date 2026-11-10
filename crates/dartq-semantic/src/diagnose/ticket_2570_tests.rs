//! The #2570 pins: usage-cluster and misc tail sweep. The import-bearing
//! shapes mount sibling libraries through the shared rig
//! (`report_ops_imports`); every pin asserts the oracle verdict a corpus
//! row pins, so a regression here is a face gap by construction.

use super::test_rig::{RigImport, report_ops_imports};
use super::*;

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

// ---------------------------------------------------------------------------
// unused_import shapes (#2570). The import-bearing rig mirrors the oracle
// corpus rows cited per test; every pin asserts the oracle verdict, so a
// regression here is a face gap by construction.
// ---------------------------------------------------------------------------

/// `UnusedImportTest.test_library_prefixed` shape: the same URI imported
/// plain and prefixed — the prefixed accesses credit the prefixed edge
/// alone, so the plain copy stays unused and reports.
#[test]
fn duplicate_uri_plain_copy_reports_unused_import() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         import 'package:t/lib1.dart' as one;\n\
         one.A a = one.A();\n",
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class A { A(); }",
            },
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: Some("one"),
                source: "class A { A(); }",
            },
        ],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        1,
        "one plain copy unused: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_extension_instance_operator_unary`
/// shape: `-C()` applies the imported extension's operator implicitly —
/// the defining import is used and stays silent.
#[test]
fn implicit_extension_operator_keeps_import_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         f() {\n\
           -C();\n\
         }\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class C { C(); }\nextension E on C {\n  void operator -() {}\n}\n",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        0,
        "operator application uses the import: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_extension_instance_setter` shape: an
/// implicit extension setter write applies the imported extension.
#[test]
fn implicit_extension_setter_keeps_import_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         f() {\n\
           C().foo = 1;\n\
         }\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class C { C(); }\nextension E on C {\n  set foo(int v) {}\n}\n",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        0,
        "setter application uses the import: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_extension_instance_indexRead` shape: an
/// implicit extension `[]` read applies the imported extension.
#[test]
fn implicit_extension_index_keeps_import_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         f() {\n\
         var x = C()[0];\n\
         }\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class C { C(); }\nextension E on C {\n  int operator [](int i) => 0;\n}\n",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        0,
        "index application uses the import: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_extension_instance_call` shape: an
/// implicit extension method invocation applies the imported extension.
#[test]
fn implicit_extension_call_keeps_import_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         f() {\n\
           C().baz();\n\
         }\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class C { C(); }\nextension E on C {\n  void baz() {}\n}\n",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        0,
        "method application uses the import: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_extension_instance_getter_fromObjectPattern`
/// shape: an object pattern's implicit getter access applies the imported
/// extension.
#[test]
fn implicit_extension_getter_keeps_import_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         f() {\n\
           switch (C()) {\n\
             case C c(: var baz): print(c);\n\
           }\n\
         }\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class C { C(); }\nextension E on C {\n  int get baz => 0;\n}\n",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        0,
        "pattern getter application uses the import: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_prefixed_samePrefix_notUsed` shape: two
/// imports share one prefix — the accesses credit the edge that exports
/// the resolved name alone, so the sibling copy stays unused and reports.
#[test]
fn same_prefix_sibling_copy_reports_unused_import() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart' as one;\n\
         import 'package:t/lib2.dart' as one;\n\
         one.A a = one.A();\n",
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: Some("one"),
                source: "class A { A(); }",
            },
            RigImport {
                uri: "package:t/lib2.dart",
                prefix: Some("one"),
                source: "class B { B(); }",
            },
        ],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        1,
        "the lib2 copy is unused: {:?}",
        messages(&r)
    );
}

/// `UnusedImportTest.test_library_systemLibrary` shape: a `dart:` import
/// whose only referenced spelling is shadowed by a local declaration —
/// the local name never credits the system import.
#[test]
fn system_import_shadowed_by_local_reports_unused() {
    let r = report_ops_imports(
        "import 'dart:x';\n\
         File? f;\n\
         class File {}\n",
        &[RigImport {
            uri: "dart:x",
            prefix: None,
            source: "class File {}",
        }],
    );
    assert_eq!(
        count_code(&r, "unused_import"),
        1,
        "the dart:x import is unused: {:?}",
        messages(&r)
    );
}
