//! The #3318 pins — the `unused_import` face's *site* contract on the
//! scopes side (`Engine::begin_import_site`).
//!
//! `report_unused_imports` pairs the written-directive site list with the
//! library's `ImportEdge` list **positionally**, and gates the whole pass
//! on the two counts agreeing. The engine registers exactly one edge per
//! directive whatever its URI shape (`DirectiveKind::Import` over
//! `unit.facts.directives`, `dartq-engine/src/pipeline.rs`), so the site
//! list must carry one entry per directive too — a directive whose site is
//! dropped does not merely go unreported, it silences every *other*
//! import in the file.
//!
//! The non-string URI head is the shape that used to drop its site: the
//! parser hands the AST a synthetic `String` token (`ensure_literal_string`
//! → `Cursor::insert_synthetic`), but a synthetic id sits past the scanned
//! table, so the engine's `import_kw.id + 1` read lands on the real token
//! (`import 5;`, an unquoted experimental URI). Upstream still carries a
//! `LibraryImport` for such a directive with an empty `relativeUriString`
//! (`imports_verifier.dart:232+` walks the directive list), so the site is
//! text-empty and zero-length at the recovered head — the anchor the corpus
//! `Unquoted-imports/*` rows pin (`14:8|0|Unused import: ''.`).
//!
//! Rows for *those* directives are not this face's: the emit skips an edge
//! with no target (`report_unused_imports`' `let Some(target) = edge.target
//! else { continue }`), and upstream's empty-URI import resolves to a real
//! (self-referential) library before it reports. The pins below therefore
//! assert the sibling-attribution contract only: whatever an untargeted
//! directive's edge does, the neighbouring imports keep their own rows and
//! their own URIs.

use super::test_rig::{RigImport, report_ops_imports};
use super::*;

/// Every `unused_import` row's message, for readable failures.
fn unused_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_import")
        .map(|d| d.message.to_string())
        .collect()
}

/// The load-bearing #3318 pin: a directive whose URI head is not a string
/// token keeps its site, so the *following* unused import still reports.
///
/// Before the fix the malformed directive contributed no site
/// (`imports.len() == 1` against `edges.len() == 3`), the alignment gate
/// bailed, and the file's whole `unused_import` face stayed silent.
#[test]
fn non_string_uri_head_does_not_silence_the_next_import() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         import 5;\n\
         void f() {}\n",
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class A { A(); }",
            },
            RigImport {
                uri: "5",
                prefix: None,
                source: "class Unrelated {}",
            },
        ],
    );
    let messages = unused_messages(&r);
    assert!(
        messages.iter().any(|m| m.contains("package:t/lib1.dart")),
        "the unused import beside a non-string URI head still reports: {messages:?}"
    );
}

/// Silent-negative half of the same contract: the sibling import is used,
/// so it keeps no row — the restored site must not turn into a false
/// positive on the import that *is* credited.
#[test]
fn non_string_uri_head_sibling_import_used_stays_silent() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         import 5;\n\
         A a = A();\n",
        &[
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class A { A(); }",
            },
            RigImport {
                uri: "5",
                prefix: None,
                source: "class Unrelated {}",
            },
        ],
    );
    let messages = unused_messages(&r);
    assert!(
        !messages.iter().any(|m| m.contains("package:t/lib1.dart")),
        "the credited import keeps no row: {messages:?}"
    );
}

/// Regression: the ordinary quoted URI path is untouched — one row, at the
/// written URI text.
#[test]
fn quoted_uri_unused_import_reports_once() {
    let r = report_ops_imports(
        "import 'package:t/lib1.dart';\n\
         void f() {}\n",
        &[RigImport {
            uri: "package:t/lib1.dart",
            prefix: None,
            source: "class A { A(); }",
        }],
    );
    let messages = unused_messages(&r);
    assert_eq!(
        messages,
        vec!["Unused import: 'package:t/lib1.dart'.".to_owned()],
        "one unused quoted import, one row carrying the written URI"
    );
}

/// Regression: the interpolated-URI arm already contributed a site (a
/// `String` token whose lexeme is an opening fragment), so the import
/// *after* it is judged — the shape that proves the site is not restricted
/// to honestly quoted URIs.
#[test]
fn interpolated_uri_import_keeps_the_following_row() {
    let r = report_ops_imports(
        "import 'a${1}c.dart';\n\
         import 'package:t/lib1.dart';\n\
         void f() {}\n",
        &[
            RigImport {
                uri: "a${1}c.dart",
                prefix: None,
                source: "class Unrelated {}",
            },
            RigImport {
                uri: "package:t/lib1.dart",
                prefix: None,
                source: "class A { A(); }",
            },
        ],
    );
    let messages = unused_messages(&r);
    assert!(
        messages.iter().any(|m| m.contains("package:t/lib1.dart")),
        "the import after an interpolated URI still reports: {messages:?}"
    );
}
