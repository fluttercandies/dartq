//! #3342 pins — the `unused_import` face and the clusters the corpus
//! audit split it into.
//!
//! Measured baseline (full corpus, 43,959 cases, `dartq analyze` against
//! the pinned 3.13.7 oracle): 68 cases where the oracle reports an
//! `unused_import` row the engine does not (MISSING) and 57 where the
//! engine reports one the oracle does not (EXTRA). The buckets, by
//! dominant root:
//!
//! - `features/_core/co19` 52 (31 EXTRA / 21 MISSING). 27 of the EXTRA
//!   rows are `dart:js_interop_unsafe` in `Augmentations/js_interop`:
//!   a plain-project probe proves the engine matches the oracle on that
//!   exact import being unused, so the divergence is the oracle's
//!   augmentation-experiment suppression, not the credit arm. The
//!   MISSING half is dominated by `Parts-with-imports` part files whose
//!   own `import` directives never reach this pass (the library-level
//!   `self.imports`/`import_edges` alignment covers the main unit only).
//! - `diagnostics/unused_import` 13 (8 EXTRA / 5 MISSING): the extension
//!   instance-member credit (EXTRA: `test_library_extension_instance_*`)
//!   and the prefixed / `dart:io` shadowed-name arms (MISSING).
//! - `features/_core/prefix` 5 EXTRA: cascades, not credit — the oracle's
//!   import verifier is suppressed by `prefix_identifier_not_followed_by_dot`
//!   rows the engine never emits, so the count gate never fires.
//! - `features/extension-methods` 5 MISSING: the extension credit is too
//!   generous for an imported extension helper the applied member did not
//!   come from.
//!
//! The first three pins below hold the arms that are green today (the
//! regression and silent-negative faces). The rest are `#[ignore]`d
//! anchors carrying the oracle verdict for each routed cluster, so the
//! lane that owns the data source can land against them.
//!
//! The rig's prelude-carrying entry point declares classes ahead of the
//! fixture, and a directive may not follow a declaration, so these pins
//! mount imports through the prelude-free entry point and declare every
//! member-bearing class they name in the imported library.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use super::test_rig::{RigImport, code_names, report_ops_experiments_with, rows};
use super::types::FileReport;

/// The sibling library every fixture imports; it declares one class so a
/// bare or prefixed reference can credit the edge.
const SIBLING: &str = "package:t/lib1.dart";
const SIBLING_SOURCE: &str = "class A {}\n";

/// Analyzes a directive-first fixture against the mounted sibling library.
fn report(source: &str, imports: &[RigImport<'_>]) -> FileReport {
    report_ops_experiments_with(source, imports, None, &[])
}

/// The single-sibling mount every fixture below shares.
fn with_sibling<'a>(source: &'a str) -> FileReport {
    report(
        source,
        &[RigImport {
            uri: SIBLING,
            prefix: None,
            source: SIBLING_SOURCE,
        }],
    )
}

/// Regression: an import whose element is referenced is credited — no row.
#[test]
fn referenced_import_stays_silent() {
    let report = with_sibling(
        "import 'package:t/lib1.dart';\n\
         A f() => A();\n",
    );
    assert!(
        code_names(&report).is_empty(),
        "a used import credits its edge; got {:#?}",
        rows(&report)
    );
}

/// Regression: a wholly unreferenced import reports once, at its URI.
#[test]
fn unreferenced_import_reports() {
    let report = with_sibling(
        "import 'package:t/lib1.dart';\n\
         void f() {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["unused_import"],
        "one row for the unused edge; got {:#?}",
        rows(&report)
    );
}

/// Silent negative: a prefixed import reached through `p.Name` is credited
/// by the prefix evidence, never by a bare-name overlap.
#[test]
fn prefixed_import_used_through_its_prefix_stays_silent() {
    let report = report(
        "import 'package:t/lib1.dart' as p;\n\
         p.A f() => p.A();\n",
        &[RigImport {
            uri: SIBLING,
            prefix: Some("p"),
            source: SIBLING_SOURCE,
        }],
    );
    assert!(
        code_names(&report).is_empty(),
        "a `p.Name` read credits the prefixed edge; got {:#?}",
        rows(&report)
    );
}

/// Anchor (routed: the prefix evidence arm): a prefixed import bound under
/// a prefix nothing reads reports while its unprefixed sibling stays
/// credited. Corpus shape
/// `UnusedImportTest/test_library_prefixed_samePrefix_notUsed`.
#[test]
#[ignore = "prefixed-unused arm: the engine credits the `as p` edge the oracle leaves unused"]
fn prefixed_import_without_a_prefix_use_reports() {
    let report = with_sibling(
        "import 'package:t/lib1.dart';\n\
         import 'package:t/lib1.dart' as p;\n\
         void f() {\n\
         \x20 A a = A();\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unused_import"],
        "the `as p` edge is unreferenced; got {:#?}",
        rows(&report)
    );
}

/// Anchor (routed: the extension-member credit pool): an import reached
/// only through an extension INSTANCE member the applied face resolves
/// credits its edge. Corpus shape
/// `UnusedImportTest/test_library_extension_instance_indexRead`.
#[test]
#[ignore = "extension instance-member credit: the applied face's members never reach the credit pool"]
fn extension_instance_member_credits_its_import() {
    let report = report(
        "import 'package:t/lib1.dart';\n\
         void f(A a) {\n\
         \x20 a[0];\n\
         }\n",
        &[RigImport {
            uri: SIBLING,
            prefix: None,
            source: "class A {}\n\
                     extension E on A {\n\
                     \x20 A operator [](A i) => i;\n\
                     }\n",
        }],
    );
    assert!(
        code_names(&report).is_empty(),
        "the applied extension member credits the import; got {:#?}",
        rows(&report)
    );
}

/// Anchor (routed: per-unit import lists): a part file that carries its own
/// `import` reports against THAT file. Corpus shape
/// `co19/LanguageFeatures/Parts-with-imports/scope_A03_t02`.
#[test]
#[ignore = "per-unit imports: the part file's own directives never reach report_unused_imports"]
fn part_file_import_reports_in_its_own_unit() {
    let report = with_sibling(
        "import 'package:t/lib1.dart';\n\
         part 'p.dart';\n\
         void f() {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["unused_import"],
        "the part's own unused import reports; got {:#?}",
        rows(&report)
    );
}

/// Anchor (routed: the shadowed system-import arm): `dart:io` stays unused
/// when the unit's bare `File` resolves to a locally imported `File` class.
/// Corpus shape `UnusedImportTest/test_library_systemLibrary`.
#[test]
#[ignore = "shadowed system import: the bare name resolves to the sibling's `File`, the dart:io edge still credits"]
fn shadowed_system_import_reports_beside_a_local_name() {
    let report = report(
        "import 'package:t/lib1.dart';\n\
         void f(A? a) {}\n",
        &[RigImport {
            uri: SIBLING,
            prefix: None,
            source: "class File {}\n",
        }],
    );
    assert_eq!(
        code_names(&report),
        ["unused_import"],
        "the sibling import is unused; got {:#?}",
        rows(&report)
    );
}
