//! #3045 pins: the part/part-of family and non-library import/export
//! emitters, locked to the `dart analyze` 3.13.7 oracle rows (conformance
//! sweep, `conformance/expected.tsv`).
//!
//! Faces covered, one pin per emitter:
//!
//! * an interpolated directive URI reports `uri_with_interpolation` at the
//!   whole literal and never resolves — the `directive.uri == None` face
//!   (`read_uri`), resolved by the URI layer as `DirectiveUriWithoutString`
//!   (file_state.dart:118) and reported at the URI node's beginToken..
//!   endToken (library_analyzer.dart:732/:954/:993);
//! * `import`/`export` of a part file reports
//!   `import_of_non_library`/`export_of_non_library` with the written
//!   selected spelling — `importedLibrarySource == null` after the
//!   missing-target arms (`_reportImportDirectiveErrors`,
//!   library_analyzer.dart:701-744 / `_reportLibraryExportDirectiveErrors`
//!   :910-955), the file-kind decision mirroring `_updateKind`
//!   (file_state.dart:928-949);
//! * `part` of a file with no part-of directive reports `part_of_non_part`
//!   with the target's canonical spelling — `includedFile.uriStr`
//!   (`pathToUri`, `package:` inside a package) — and a valid import stays
//!   silent;
//! * a name-form part-of that fails to claim its includer reports
//!   `part_of_different_library` ("Expected this library to be part of
//!   '{expectedName}', not '{actualName}'.") or `part_of_unnamed_library`
//!   against an unnamed includer — the
//!   `partElementUri is! DirectiveUriWithUnitImpl` arm
//!   (library_analyzer.dart:1037-1058); a URI-form part-of reports the
//!   enclosing/included canonical URIs through the same template.

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
/// same shape the facade tests' fixture builds (package `t`, language
/// version 3.5, zero experiment grants — the sweep's default feature set,
/// enhanced-parts off), trimmed to what [`Engine::new`] binds.
fn pin_engine() -> (PathBuf, Engine) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3045-{}-{}",
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
        // The sweep chunks' pub shape — `packageUri` explicit, so the
        // canonical spellings strip `lib/` exactly as the oracle rows carry
        // them (`package:t/a.dart`).
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
/// the full diagnostic set — no semantic diagnoser: the emitters under pin
/// are engine faces.
fn analyze_files(files: &[(&str, &str)]) -> (PathBuf, Engine, Vec<Diagnostic>) {
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
    (root, engine, diagnostics)
}

const INTERPOLATION_MESSAGE: &str = "URIs can't use string interpolation.";

/// `import 'a${b}.dart';` + `part 'p${x}.dart';` — both interpolated URI
/// forms report `uri_with_interpolation` at their whole literal, in
/// directive order, and neither resolves (no `uri_does_not_exist`, no
/// target face). Upstream: a stringless URI's `stringValue` is null, the
/// only shape treated as carrying no string (ast.dart:51023), reported at
/// the URI node (library_analyzer.dart:736-744 imports, :951-955 exports,
/// :990-993 part includes).
#[test]
fn interpolated_uris_report_at_the_whole_literal_and_never_resolve() {
    const TEST: &str = "import 'a${b}.dart';\npart 'p${x}.dart';\nint f() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST)]);
    assert_eq!(
        rows(&diagnostics, "uri_with_interpolation"),
        vec![
            (7, 12, INTERPOLATION_MESSAGE.to_owned()),
            (26, 12, INTERPOLATION_MESSAGE.to_owned()),
        ],
        "the two interpolated literals: {diagnostics:?}"
    );
    for code in ["uri_does_not_exist", "invalid_uri", "part_of_non_part"] {
        assert!(
            rows(&diagnostics, code).is_empty(),
            "{code} must stay silent — an interpolated URI never resolves: {diagnostics:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'a.dart';` where `a.dart` is a part file (`part of 'test.dart';`)
/// — `import_of_non_library` at the written literal. Oracle row:
/// `ERROR|import_of_non_library|lib/test.dart|2:8|8|The imported library
/// 'a.dart' can't have a part-of directive.` (library_analyzer.dart
/// :728-732, `importedLibrarySource == null`; the part-file kind is the
/// target's own `_updateKind` classification, file_state.dart:928-949).
/// The part file's own analysis stays silent — its part-of URI only
/// classifies the file (file_state.dart:938-953), never a diagnostic.
#[test]
fn import_of_a_part_file_reports_import_of_non_library() {
    const TEST: &str = "import 'a.dart';\nint f() => 1;\n";
    const A: &str = "part of 'test.dart';\nint g() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("a.dart", A)]);
    assert_eq!(
        rows(&diagnostics, "import_of_non_library"),
        vec![(
            7,
            8,
            "The imported library 'a.dart' can't have a part-of directive.".to_owned()
        )],
        "the import of a part file: {diagnostics:?}"
    );
    assert!(
        rows(&diagnostics, "uri_does_not_exist").is_empty(),
        "the target exists — no URI-layer row: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `export 'a.dart';` against a part file — `export_of_non_library`, the
/// export twin of the face above (library_analyzer.dart:910-955, the same
/// `importedLibrarySource == null` decision on the export state). The
/// deferred spelling rides the same face (oracle
/// ImportOfNonLibraryTest/test_deferred): deferredness never gates it.
#[test]
fn export_of_a_part_file_reports_export_of_non_library() {
    const TEST: &str = "export 'a.dart';\nint f() => 1;\n";
    const A: &str = "part of 'test.dart';\nint g() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("a.dart", A)]);
    assert_eq!(
        rows(&diagnostics, "export_of_non_library"),
        vec![(
            7,
            8,
            "The exported library 'a.dart' can't have a part-of directive.".to_owned()
        )],
        "the export of a part file: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `import 'a.dart' deferred as a;` against a part file — the row still
/// reports at the URI literal (oracle
/// `ImportOfNonLibraryTest/test_deferred`).
#[test]
fn deferred_import_of_a_part_file_still_reports() {
    const TEST: &str = "import 'a.dart' deferred as a;\nint f() => 1;\n";
    const A: &str = "part of 'test.dart';\nint g() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("a.dart", A)]);
    assert_eq!(
        rows(&diagnostics, "import_of_non_library"),
        vec![(
            7,
            8,
            "The imported library 'a.dart' can't have a part-of directive.".to_owned()
        )],
        "the deferred import of a part file: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A valid import of a real library file stays silent on the whole
/// non-library face — the check sits strictly behind the target's
/// file-kind question (library_analyzer.dart:701-727).
#[test]
fn import_of_a_library_file_stays_silent() {
    const TEST: &str = "import 'a.dart';\nint f() => 1;\n";
    const A: &str = "int x = 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("a.dart", A)]);
    for code in [
        "import_of_non_library",
        "export_of_non_library",
        "uri_does_not_exist",
        "uri_with_interpolation",
    ] {
        assert!(
            rows(&diagnostics, code).is_empty(),
            "{code} must stay silent — the target is a library: {diagnostics:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `part 'a.dart';` where `a.dart` carries no part-of directive —
/// `part_of_non_part` with the target's canonical spelling. Oracle row:
/// `ERROR|part_of_non_part|lib/test.dart|3:6|8|The included part
/// 'package:test/a.dart' must have a part-of directive.`
/// (library_analyzer.dart:1010-1017, `FileState(exists: true)` arm,
/// `includedFile.uriStr` — `package:` inside a package).
#[test]
fn part_of_a_non_part_file_reports_part_of_non_part() {
    const TEST: &str = "part 'a.dart';\n";
    const A: &str = "int x = 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("a.dart", A)]);
    assert_eq!(
        rows(&diagnostics, "part_of_non_part"),
        vec![(
            5,
            8,
            "The included part 'package:t/a.dart' must have a part-of directive.".to_owned()
        )],
        "the include of a non-part file: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A named library including a name-form part that claims another name —
/// `part_of_different_library` with the two library names. Oracle row:
/// `Expected this library to be part of 'foo', not 'bar'.`
/// (library_analyzer.dart:1044-1057, `PartOfNameFileKind` under the
/// enhanced-parts horizon — this fixture runs without the experiment, the
/// sweep's default set).
#[test]
fn name_form_mismatch_reports_part_of_different_library() {
    const TEST: &str = "library foo;\npart 'p.dart';\n";
    const P: &str = "part of bar;\nint g() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("p.dart", P)]);
    assert_eq!(
        rows(&diagnostics, "part_of_different_library"),
        vec![(
            18,
            8,
            "Expected this library to be part of 'foo', not 'bar'.".to_owned()
        )],
        "the name-form claim mismatch: {diagnostics:?}"
    );
    assert!(
        rows(&diagnostics, "part_of_name").is_empty(),
        "enhanced-parts is off — no parser-tier part_of_name row: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An unnamed library including a name-form part —
/// `part_of_unnamed_library` with the part's claimed name. Oracle row:
/// `The library is unnamed. A URI is expected, not a library name 'lib',
/// in the part-of directive.` (library_analyzer.dart:1047-1051, the empty
/// `libraryName` arm).
#[test]
fn name_form_against_an_unnamed_includer_reports_part_of_unnamed_library() {
    const TEST: &str = "part 'p.dart';\n";
    const P: &str = "part of lib;\nint g() => 1;\n";

    let (root, _engine, diagnostics) = analyze_files(&[("test.dart", TEST), ("p.dart", P)]);
    assert_eq!(
        rows(&diagnostics, "part_of_unnamed_library"),
        vec![(
            5,
            8,
            "The library is unnamed. A URI is expected, not a library name 'lib', \
             in the part-of directive."
                .to_owned()
        )],
        "the unnamed-includer row: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A URI-form part-of claiming a different file —
/// `part_of_different_library` with the enclosing and included canonical
/// URIs through the same template (library_analyzer.dart:1057-1058,
/// `PartOfUriFileKind`, `enclosingFile.file.uriStr` /
/// `includedFile.uriStr`).
#[test]
fn uri_form_mismatch_reports_part_of_different_library_with_uris() {
    const TEST: &str = "library t;\npart 'p.dart';\n";
    const P: &str = "part of 'other.dart';\nint g() => 1;\n";
    const OTHER: &str = "int h() => 2;\n";

    let (root, _engine, diagnostics) =
        analyze_files(&[("test.dart", TEST), ("p.dart", P), ("other.dart", OTHER)]);
    assert_eq!(
        rows(&diagnostics, "part_of_different_library"),
        vec![(
            16,
            8,
            "Expected this library to be part of 'package:t/test.dart', \
             not 'package:t/p.dart'."
                .to_owned()
        )],
        "the URI-form claim mismatch: {diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
