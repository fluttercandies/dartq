//! #3403 pins: pseudo keywords in directive name slots — `is_name_token`
//! (l1.rs) must take them like any identifier, because upstream
//! `ensureIdentifier` (`identifier_context_impl.dart:130-165`) accepts every
//! `next.isIdentifier` token in `CombinatorIdentifierContext`,
//! `LibraryIdentifierContext`, and `DottedNameIdentifierContext` alike; the
//! next-declaration-head refusal lives in the keep-set/`refuses_name`
//! boundary, and no declaration head is a pseudo keyword.
//!
//! Locked to the `dart analyze` 3.13.7 oracle rows. Before the gate fix a
//! pseudo-keyword combinator name was dropped, so the clause read no names
//! (`show` imported nothing) and every usage site reported `undefined_*`
//! EXTRA; the same drop silenced `undefined_shown_name`/
//! `undefined_hidden_name` (the name never reached the clause).
//!
//! Faces covered:
//!
//! * `show when; when()` — the shown pseudo name resolves at usage (the
//!   bug's user-visible face);
//! * `show when hide base;` — both clauses read, the shown name resolves;
//! * `hide sync;` — the hidden name is read (one `undefined_hidden_name`
//!   when the target does not export it);
//! * `show show;` — the clause keyword itself as a name (one
//!   `undefined_shown_name`).

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
/// same shape the #3045/#3066/#3103/#3146 pin fixtures build (package `t`,
/// language version 3.5), with the default diagnoser wired so a failed
/// name resolution reports at the usage site.
fn pin_engine() -> (PathBuf, Engine) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3403-{}-{}",
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
        semdiag: Some(crate::semdiag::diagnoser()),
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

/// Every `undefined_*` row (any resolver face) of a diagnostic set — a
/// resolved combinator name keeps this empty.
fn undefined_rows(diagnostics: &[Diagnostic]) -> Vec<(String, u32, u32)> {
    let mut rows: Vec<(String, u32, u32)> = diagnostics
        .iter()
        .filter(|row| row.code.name.starts_with("undefined"))
        .map(|row| (row.code.name.to_string(), row.range.offset, row.range.length))
        .collect();
    rows.sort();
    rows
}

/// `import 'existing.dart' show when; void main() { when(); }` — `when` is
/// a pseudo keyword and a legal shown name (`parseIdentifierList` :1250
/// takes it through `ensureIdentifier`), so the clause resolves and the
/// usage is silent. The oracle reports nothing; before the gate fix the
/// clause read no names and the usage reported `undefined_*` EXTRA.
#[test]
fn shown_pseudo_name_resolves_at_usage() {
    const TEST: &str = "import 'existing.dart' show when;\nvoid main() { when(); }\n";
    const EXISTING: &str = "void when() {}\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST), ("existing.dart", EXISTING)]);
    assert_eq!(
        undefined_rows(&diagnostics),
        Vec::new(),
        "the shown pseudo name resolves: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'existing.dart' show when hide base;` — both clause name lists
/// read (`when` shown resolves; `base` hidden and unreferenced). The
/// oracle reports nothing.
#[test]
fn shown_and_hidden_pseudo_names_resolve() {
    const TEST: &str = "import 'existing.dart' show when hide base;\nvoid main() { when(); }\n";
    const EXISTING: &str = "void when() {}\nvoid base() {}\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST), ("existing.dart", EXISTING)]);
    assert_eq!(
        undefined_rows(&diagnostics),
        Vec::new(),
        "both clauses read their pseudo names: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'existing.dart' hide sync;` — `sync` is a pseudo keyword and a
/// hidden name the target does not export: DeadCodeVerifier reports
/// `undefined_hidden_name` at the written token (dead_code_verifier.dart
/// :58-70), which proves the name reached the clause.
#[test]
fn hidden_pseudo_name_is_read() {
    const TEST: &str = "import 'existing.dart' hide sync;\n";
    const EXISTING: &str = "int keep = 0;\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST), ("existing.dart", EXISTING)]);
    let existing_uri = format!("file://{}", root.join("lib/existing.dart").display());
    assert_eq!(
        rows(&diagnostics, "undefined_hidden_name"),
        vec![(
            28,
            4,
            format!(
                "The library '{existing_uri}' doesn't export a member with the hidden name \
                 'sync'."
            )
        )],
        "the hidden pseudo name is a name: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'existing.dart' show show;` — the clause keyword itself in name
/// position is the shown name (`CombinatorIdentifierContext` takes it), so
/// the oracle reports `undefined_shown_name` at the second `show` token.
#[test]
fn clause_keyword_as_shown_name_is_read() {
    const TEST: &str = "import 'existing.dart' show show;\n";
    const EXISTING: &str = "int keep = 0;\n";

    let (root, diagnostics) = analyze_files(&[("test.dart", TEST), ("existing.dart", EXISTING)]);
    let existing_uri = format!("file://{}", root.join("lib/existing.dart").display());
    assert_eq!(
        rows(&diagnostics, "undefined_shown_name"),
        vec![(
            28,
            4,
            format!(
                "The library '{existing_uri}' doesn't export a member with the shown name \
                 'show'."
            )
        )],
        "the second `show` is the shown name: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
