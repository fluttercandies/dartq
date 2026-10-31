//! #3103 pins: the directive recovery boundary — a directive that lost its
//! `;` must end at the next directive head instead of consuming it, locked
//! to the `dart analyze` 3.13.7 oracle rows (conformance family
//! `diagnostics/uri_does_not_exist`, `ExportDirectivesTest` /
//! `ImportDirectivesTest` / `LibraryDirectivesTest` tails).
//!
//! Faces covered, one pin per clause-tail shape:
//!
//! * `export '' export 'a.dart';` — the trailing export keeps its own
//!   directive face, so `a.dart` reaches the URI layer and reports
//!   `uri_does_not_exist` (upstream `parseExport`'s `ensureSemicolon`
//!   (parser_impl.dart:1180-1189) splices the `;` in front of the head and
//!   returns, leaving the head to the enclosing declaration loop);
//! * `import 'b.dart' import 'c.dart';` — both URIs report (upstream
//!   `parseImport` :960-981 hands the `;`-less tail to `parseImportRecovery`
//!   :982, whose extraneous-token boundary set :1008-1017 never consumes a
//!   directive keyword and whose no-progress branch inserts the synthetic
//!   `;` at :1078-1081);
//! * a conditional-URI tail (`if (…) 'y.dart' export 'a.dart';`);
//! * a cross-family head (`export 'x.dart' part 'p.dart';`) — the part
//!   directive's own canonical-URI row survives;
//! * a chain of two heads (`import 'a.dart' import 'b.dart' import
//!   'c.dart';`) — the boundary re-enters repeatedly;
//! * built-in identifiers as combinator names (`show export`, `hide
//!   library`) stay names, not directive heads — upstream
//!   `parseIdentifierList` :1250 walks them through `ensureIdentifier`
//!   (`IdentifierContext.combinator`), which accepts built-in identifiers
//!   silently, so `hide library` is a hidden name and reports nothing.
//!
//! The library family needs no pin here: `library lib1 library lib2;` is
//! already recovered (the official `parseLibraryName` ends in
//! `ensureSemicolon` too), and its `library_directive_not_first` rows come
//! from the parser face rather than the directive-fact walk.

use super::*;
use dartq_pub::{LockFile, Pubspec, build_graph};

/// The rows of one code, as (offset, length, message) — every fixture here
/// carries its rows in a single file, so the file identity is not part of
/// the comparison.
fn rows(diagnostics: &[Diagnostic], code: &str) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length, row.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// A throwaway single-package workspace + synthetic core-only SDK — the
/// same shape the #3045/#3066 pin fixtures build (package `t`, language
/// version 3.5).
fn pin_engine() -> (PathBuf, Engine) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3103-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let write = |relative: &str, content: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    };
    write(
        "pubspec.yaml",
        "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
    );
    write(
        "pubspec.lock",
        "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
    );
    write(
        ".dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"languageVersion": "3.5", "name": "t", "packageUri": "lib/", "rootUri": "../"}]}"#,
    );
    write("sdk/version", "3.13.1\n");
    write(
        "sdk/lib/libraries.json",
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );
    write("sdk/lib/core/core.dart", "library dart.core;\n");
    // Canonicalize before the pub faces resolve it (the #287 spelling
    // universe — macOS /var vs /private/var).
    let root = root.canonicalize().expect("fixture root resolves");
    let config_dir = root.join(".dart_tool");
    let config = Arc::new(
        PackageConfig::parse(
            &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
        )
        .expect("valid config"),
    );
    let lock =
        LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("valid lock");
    let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
        .expect("valid pubspec");
    let graph = build_graph(
        &config,
        &lock,
        &root_pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    )
    .expect("graph");
    let engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages: PackageTable::build(&config, &graph, &config_dir).expect("table"),
        pub_config: Arc::clone(&config),
        config_dir,
        context_roots: Vec::new(),
        semdiag: None,
        target: DEFAULT_TARGET.to_owned(),
        declared_variables: FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    (root, engine)
}

/// Analyzes a fresh workspace holding `files` (lib-relative) and returns
/// the workspace root (for canonical-URI expectations) with the full
/// diagnostic set.
fn analyze_files(files: &[(&str, &str)]) -> (PathBuf, Vec<Diagnostic>) {
    let (root, mut engine) = pin_engine();
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).expect("lib dir");
    for (name, text) in files {
        std::fs::write(lib.join(name), text).expect("fixture file");
    }
    engine.discover_workspace().expect("discover");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    (root, diagnostics)
}

/// The written-URI row message (`uri_does_not_exist` keeps the source
/// spelling, library_analyzer.dart:732/:993/:1031).
fn missing_uri(uri: &str) -> String {
    format!("Target of URI doesn't exist: '{uri}'.")
}

/// `export '' export 'a.dart';` — oracle: `expected_token` at the empty
/// literal, then `uri_does_not_exist` for `a.dart` at 1:18. The empty URI
/// composes to the container itself and reports nothing.
#[test]
fn unterminated_export_keeps_the_following_export() {
    const TEST: &str = "export '' export 'a.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(17, 8, missing_uri("a.dart"))],
        "the trailing export is a directive of its own: {diagnostics:?}"
    );
    assert!(
        rows(&diagnostics, "duplicate_export").is_empty(),
        "two distinct URIs are no duplicate: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'b.dart' import 'c.dart';` — oracle: one row per written URI
/// (1:8 and 1:24), the first import carrying the recovery's
/// `expected_token` on the parser face.
#[test]
fn unterminated_import_keeps_the_following_import() {
    const TEST: &str = "import 'b.dart' import 'c.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("b.dart")),
            (23, 8, missing_uri("c.dart"))
        ],
        "both imports keep their URI face: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'a.dart' import 'b.dart' import 'c.dart';` — the boundary is
/// re-entrant: every later head starts its own directive, so all three
/// URIs report (the official recovery only ever consumes ONE token without
/// progress, then splices the `;` — parser_impl.dart:1078-1081).
#[test]
fn a_chain_of_unterminated_imports_recovers_each_head() {
    const TEST: &str = "import 'a.dart' import 'b.dart' import 'c.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("a.dart")),
            (23, 8, missing_uri("b.dart")),
            (39, 8, missing_uri("c.dart")),
        ],
        "every head is its own directive: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `export 'x.dart' if (dart.library.io) 'y.dart' export 'a.dart';` —
/// oracle: `uri_does_not_exist` for `x.dart` at 1:8 and for `a.dart` at
/// 1:54, with `expected_token` on the conditional tail's literal (1:38).
/// The conditional's candidate URI never carries a row (it only selects).
#[test]
fn conditional_uri_tail_keeps_the_following_export() {
    const TEST: &str = "export 'x.dart' if (dart.library.io) 'y.dart' export 'a.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("x.dart")),
            (53, 8, missing_uri("a.dart"))
        ],
        "the trailing export survives the conditional tail: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `export 'x.dart' part 'p.dart';` — a cross-family head starts its own
/// directive too; the part row carries the TARGET's canonical spelling
/// (`package:` inside a package — the part re-spelling face,
/// file_state.dart:938-953), not the written one.
#[test]
fn part_head_after_an_unterminated_export_is_recovered() {
    const TEST: &str = "export 'x.dart' part 'p.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("x.dart")),
            (21, 8, missing_uri("package:t/p.dart")),
        ],
        "the part directive keeps its own URI face: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `export 'x.dart' show y export 'a.dart';` — a combinator's name list is
/// `identifier (',' identifier)*` (parseIdentifierList :1250), so the bare
/// head after the name is the NEXT directive, not another name. Oracle:
/// `uri_does_not_exist` for `x.dart` at 1:8 and `a.dart` at 1:31.
#[test]
fn combinator_tail_keeps_the_following_export() {
    const TEST: &str = "export 'x.dart' show y export 'a.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("x.dart")),
            (30, 8, missing_uri("a.dart"))
        ],
        "`show y` does not take the trailing head as a name: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'x.dart' show export import 'a.dart';` — the name position takes
/// the built-in identifier `export`, and the following `import` starts its
/// own directive. Oracle: `uri_does_not_exist` for `x.dart` at 1:8 and
/// `a.dart` at 1:36 (plus the parser's own recovery rows on the tail).
#[test]
fn built_in_name_then_head_keeps_both_directives() {
    const TEST: &str = "import 'x.dart' show export import 'a.dart';\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![
            (7, 8, missing_uri("x.dart")),
            (35, 8, missing_uri("a.dart"))
        ],
        "the shown name and the trailing import are distinct: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Built-in identifiers are legal combinator names (`parseIdentifierList`
/// :1250 → `ensureIdentifier(…, IdentifierContext.combinator)`), so a
/// directive head spelled in name position is a NAME, never the next
/// directive's head. Oracle: `import 'existing.dart' show export;` reports
/// `undefined_shown_name` at the written `export` token (1:29 here), and
/// `import 'missing.dart' hide library;` stays silent behind the
/// nonexistent-target gate.
#[test]
fn built_in_identifier_combinator_names_are_names() {
    const TEST: &str = "import 'existing.dart' show export;\nimport 'missing.dart' hide library;\n";
    const EXISTING: &str = "int greet = 0;\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST), ("existing.dart", EXISTING)]);
    let existing_uri = format!("file://{}", root.join("lib/existing.dart").display());
    assert_eq!(
        rows(&diagnostics, "undefined_shown_name"),
        vec![(
            28,
            6,
            format!(
                "The library '{existing_uri}' doesn't export a member with the shown name \
                 'export'."
            )
        )],
        "`export` in name position is a shown name: {diagnostics:?}"
    );
    assert!(
        rows(&diagnostics, "undefined_hidden_name").is_empty(),
        "the unresolved target suppresses the hidden-name face: {diagnostics:?}"
    );
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(43, 14, missing_uri("missing.dart"))],
        "the second directive is `import 'missing.dart'`, not a `library` head: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
