//! #3066 pins: the `uri_does_not_exist` emitter faces, locked to the
//! `dart analyze` 3.13.7 oracle rows (conformance sweep,
//! `conformance/expected.tsv`, family `diagnostics/uri_does_not_exist`).
//!
//! Faces covered, one pin per URI-layer miss arm:
//!
//! * an unknown `dart:` library (`import 'dart:foo';`) reports
//!   `uri_does_not_exist` at the written literal — `flat_library_to_path`
//!   misses (the oracle's flat `libraries.dart` map,
//!   `internalMapDartUri`, sdk.dart:90-130: source is null → the
//!   URI-layer contract reports the miss from the source spelling);
//! * an unknown `package:` target reports the same row from the source
//!   spelling — the lib-dir universe miss lands in `land_package`;
//! * an unknown-scheme target (`import 'foo:bar';`) — `Composed::Other`,
//!   the same row;
//! * existing targets (a relative file, a known `dart:` library) stay
//!   silent;
//! * a missing `part` target reports the row with the target's canonical
//!   `file://` spelling — the part re-spelling face (`FileState.uriStr`,
//!   `pathToUri`; the oracle message carries the canonical URI, unlike the
//!   import faces which carry the written one);
//! * a URI-form `part of` whose target is missing never carries a URI
//!   code — the part-of URI only classifies the file
//!   (file_state.dart:938-953); whatever else fires, the part-of literal
//!   itself stays out of the `uri_does_not_exist` rows.

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
/// same shape the #3045 pin fixture builds (package `t`, language version
/// 3.5), plus the `dart:core` source file so a known-library import can
/// resolve silently.
fn pin_engine() -> (PathBuf, Engine) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3066-{}-{}",
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
    // `dart:core` resolves through the flat library table to this file —
    // present, so the known-library import is silent.
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
/// the full diagnostic set.
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

/// `import 'dart:foo';` — an unknown `dart:` library reports
/// `uri_does_not_exist` at the written literal (oracle sweep row
/// `ERROR|uri_does_not_exist|lib/test.dart|2:8|10|Target of URI doesn't
/// exist: 'dart:foo'.`). The miss rides the flat library table lookup —
/// no `--sdk-path`-shaped table swap can move it.
#[test]
fn unknown_dart_library_reports_at_the_literal() {
    let (_root, diagnostics) = analyze_files(&[("test.dart", "import 'dart:foo';\n")]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(7, 10, "Target of URI doesn't exist: 'dart:foo'.".to_owned())],
        "the unknown dart: library: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}

/// `import 'package:nonexist/x.dart';` — an unknown package (the lib-dir
/// universe miss) reports the same row from the source spelling (oracle
/// sweep rows carry `'package:…'` verbatim).
#[test]
fn unknown_package_target_reports_the_written_spelling() {
    let (_root, diagnostics) =
        analyze_files(&[("test.dart", "import 'package:nonexist/x.dart';\n")]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(
            7,
            25,
            "Target of URI doesn't exist: 'package:nonexist/x.dart'.".to_owned()
        )],
        "the unknown package: target: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}

/// `import 'foo:bar';` — an unknown scheme composes to `Composed::Other`
/// and reports the same row at the literal.
#[test]
fn unknown_scheme_reports_at_the_literal() {
    let (_root, diagnostics) = analyze_files(&[("test.dart", "import 'foo:bar';\n")]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(7, 9, "Target of URI doesn't exist: 'foo:bar'.".to_owned())],
        "the unknown-scheme target: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}

/// Existing targets stay silent: a present relative file and a known
/// `dart:` library (`dart:core` through the flat table) produce no
/// URI-layer row at all (oracle sweep `doc_good` rows).
#[test]
fn existing_targets_stay_silent() {
    let (_root, diagnostics) = analyze_files(&[
        (
            "test.dart",
            "import 'a.dart';\nimport 'dart:core';\nint f() => 1;\n",
        ),
        ("a.dart", "library a;\n"),
    ]);
    assert!(
        rows(&diagnostics, "uri_does_not_exist").is_empty(),
        "existing targets must stay silent: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}

/// `part 'missing.dart';` — the part-inclusion miss reports
/// `uri_does_not_exist` with the target's **canonical** spelling — inside
/// the package lib dir that is `package:t/missing.dart`, outside it the
/// `file://` URI (oracle row `test_getResolvedUnit_part_doesNotExist_lints#0`:
/// `ERROR|uri_does_not_exist|lib/test.dart|3:6|8|Target of URI doesn't
/// exist: 'package:test/a.dart'.`; the sweep's `test/`-dir part rows carry
/// the `file://<chunk>/…` spelling). Unlike the import faces above, the
/// message carries the re-spelled target URI, not the written one
/// (`FileState.uriStr`).
#[test]
fn missing_part_target_reports_the_canonical_spelling() {
    let (_root, diagnostics) = analyze_files(&[("test.dart", "part 'missing.dart';\n")]);
    assert_eq!(
        rows(&diagnostics, "uri_does_not_exist"),
        vec![(
            5,
            14,
            "Target of URI doesn't exist: 'package:t/missing.dart'.".to_owned()
        )],
        "the missing part target: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}

/// A URI-form `part of` whose target is missing never carries a URI code:
/// the part-of URI only classifies the file (file_state.dart:938-953), so
/// the part-of literal stays out of the `uri_does_not_exist` rows however
/// the claim failure reports.
#[test]
fn part_of_uri_miss_stays_out_of_uri_rows() {
    let (_root, diagnostics) = analyze_files(&[
        ("test.dart", "part 'p.dart';\nint f() => 1;\n"),
        ("p.dart", "part of 'gone.dart';\nint g() => 1;\n"),
    ]);
    assert!(
        rows(&diagnostics, "uri_does_not_exist").is_empty(),
        "the part-of URI miss must stay out of the uri rows: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&_root);
}
