//! A foreign type alias's invocation face: the imported alias whose
//! right-hand side lives behind an unresolvable import
//! (`typedef TCG = shared.TCG<int>;` in another unit — the
//! `generic_inferrer.dart` census shape) reports
//! `invocation_of_non_function` at the head exactly like the same-file
//! spelling; the right-hand side's groundedness decides over the
//! graph-wide container face (the alias's own top-level declaration is
//! not a container). Oracle provenance: `dart analyze` 3.13.1.

use super::test_rig::{RigImport, code_names, report_ops_imports};

/// `TCG()` over an imported alias whose RHS head nothing declares
/// reports — the cross-file arm of the alias-invocation face.
#[test]
fn foreign_ungrounded_alias_invocation_reports_non_function() {
    let report = report_ops_imports(
        "// the rig auto-imports 'package:t/aliased.dart'\n\
         void f() {\n\
         \x20 TCG();\n\
         }\n",
        &[RigImport {
            uri: "package:t/aliased.dart",
            prefix: None,
            source: "typedef TCG = shared.TCG<int>;\n",
        }],
    );
    assert_eq!(code_names(&report), ["invocation_of_non_function"]);
}

/// The grounded control: an imported alias of a real class is the
/// class's creation — silent, never the invocation face.
#[test]
fn foreign_grounded_alias_invocation_stays_silent() {
    let report = report_ops_imports(
        "// the rig auto-imports 'package:t/aliased.dart'\n\
         void f() {\n\
         \x20 Real();\n\
         }\n",
        &[RigImport {
            uri: "package:t/aliased.dart",
            prefix: None,
            source: "class Impl {}\ntypedef Real = Impl;\n",
        }],
    );
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.to_string())
            .collect::<Vec<_>>()
    );
}
