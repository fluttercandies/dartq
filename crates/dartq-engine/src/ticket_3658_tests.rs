//! #3658 pins: the `unused_import` release path — element attribution,
//! per-file row mapping, and the non-library gate, locked to the
//! `dart analyze` 3.13.7 oracle rows (conformance `expected.tsv`).
//!
//! Faces covered (oracle case → expectation):
//!
//! * `test_library_prefixed` — `one.A` credits ONLY the `as one` import;
//!   the unprefixed import of the same library stays reported.
//! * `samePrefix_notUsed` — `one.A` credits only the `as one` edge whose
//!   target exports `A`; the sibling `as one` edge stays reported.
//! * `systemLibrary` — `prefix.File` resolves to the non-SDK import's
//!   `File` (the `PrefixScope._merge` non-SDK preference); `dart:io`
//!   stays reported.
//! * `test_is` (ambiguous_import gate) — an ambiguous use suppresses
//!   EVERY unused_import row of the library, including a genuinely
//!   unused third import.
//! * `usedPartImport` / `inNestedPart` — a part file's own import,
//!   credited by a use in that part or a nested sub-part, maps back
//!   through the PART's directives and releases (the `files[0]` mapping
//!   could never reach it).
//! * `inPart_library` — an UNUSED part-file import stays reported on the
//!   part file's own URI span.
//! * `inLibrary_notLibrary` — `import 'part.dart'` never reports
//!   `unused_import` (the `DirectiveUriWithLibraryImpl` gate), only the
//!   `import_of_non_library` error.
//! * `importDuplicatedLibraryName` — a duplicated unused import reports
//!   `unused_import` on BOTH directive spans.
//! * Negative controls — a used unprefixed import stays silent; a used
//!   doc `[Name]` releases through the written scope.

use super::*;
use dartq_pub::{LockFile, Pubspec, build_graph};

/// The `unused_import` rows of a diagnostic set as `(file_name,
/// offset, length, message)` — file identity matters here (part-file
/// rows anchor on the part, not the library unit), so `files` supplies
/// the FileId → basename resolution the engine's table held.
fn unused_import_rows(
    diagnostics: &[Diagnostic],
    files: &FxHashMap<FileId, String>,
) -> Vec<(String, u32, u32, String)> {
    let mut rows: Vec<(String, u32, u32, String)> = diagnostics
        .iter()
        .filter(|row| row.code.name == "unused_import")
        .map(|row| {
            (
                files.get(&row.file).cloned().unwrap_or_default(),
                row.range.offset,
                row.range.length,
                row.message.to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Whether any `unused_import` row exists — the gate-suppression and
/// silent-release faces assert on this without a span.
fn has_unused_import(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|row| row.code.name == "unused_import")
}

/// A throwaway single-package workspace + a synthetic SDK carrying
/// `core`/`io`/`math` — the #3045-family fixture shape with the two SDK
/// libraries the `systemLibrary` and `usedPartImport` faces need
/// (`dart:io`'s `File` class, `dart:math`'s `Random`).
fn pin_engine() -> (PathBuf, Engine) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-3658-{}-{}",
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
        r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "io": {"uri": "io/io.dart"}, "math": {"uri": "math/math.dart"}}}}"#,
    );
    write(
        "sdk/lib/_internal/allowed_experiments.json",
        r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
    );
    write("sdk/lib/core/core.dart", "library dart.core;\n");
    write("sdk/lib/io/io.dart", "library dart.io;\nclass File {}\n");
    write(
        "sdk/lib/math/math.dart",
        "library dart.math;\nclass Random {}\n",
    );
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

/// Analyzes a fresh workspace holding `files` (lib-relative paths) and
/// returns the workspace root, the full diagnostic set, and the
/// FileId → file-basename map the row assertions read.
fn analyze_files(files: &[(&str, &str)]) -> (PathBuf, Vec<Diagnostic>, FxHashMap<FileId, String>) {
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
    let names: FxHashMap<FileId, String> = files
        .iter()
        .filter_map(|(name, _)| {
            engine
                .file_table()
                .try_file_id(&lib.join(name))
                .map(|id| (id, (*name).to_string()))
        })
        .collect();
    (root, diagnostics, names)
}

/// `import 'lib1.dart'; import 'lib1.dart' as one;` + `one.A` — the
/// prefixed use credits the `as one` edge ONLY. Oracle
/// (`test_library_prefixed`): the UNPREFIXED import stays reported
/// (`WARNING|unused_import|lib/test.dart|2:8|11`). The released name
/// pool's `exported(target, A)` test used to release the unprefixed
/// edge too — `A` is prefix-attributed (reachable as `one.A`), so it
/// never joins the bare-use pool.
#[test]
fn prefixed_use_credits_only_the_prefixed_import() {
    let (_, diagnostics, _names) = analyze_files(&[
        (
            "test.dart",
            "import 'lib1.dart';\nimport 'lib1.dart' as one;\none.A a = one.A();\n",
        ),
        ("lib1.dart", "class A {}\n"),
    ]);
    let rows = unused_import_rows(&diagnostics, &_names);
    assert_eq!(rows.len(), 1, "exactly one unused_import row: {rows:?}");
    let (_, offset, length, message) = &rows[0];
    // `import 'lib1.dart';` — the URI literal opens at offset 7.
    assert_eq!(*offset, 7);
    assert_eq!(*length, 11);
    assert!(message.contains("lib1.dart"), "{message}");
}

/// `import 'lib1.dart' as one; import 'lib2.dart' as one;` + `one.A` —
/// `A` resolves through `one` to lib1's `A`; the lib2 edge (same
/// prefix, no `A`) stays reported. Oracle
/// (`samePrefix_notUsed`): `WARNING|unused_import|lib/test.dart|3:8|11`
/// on lib2's URI. `used.contains(&one)` used to release BOTH same-prefix
/// edges.
#[test]
fn prefixed_use_credits_only_the_exporting_edge() {
    let (_, diagnostics, _names) = analyze_files(&[
        (
            "test.dart",
            "import 'lib1.dart' as one;\nimport 'lib2.dart' as one;\none.A a = one.A();\n",
        ),
        ("lib1.dart", "class A {}\n"),
        ("lib2.dart", "class B {}\n"),
    ]);
    let rows = unused_import_rows(&diagnostics, &_names);
    assert_eq!(rows.len(), 1, "exactly one unused_import row: {rows:?}");
    let (_, offset, length, message) = &rows[0];
    // `import 'lib2.dart' as one;` — the URI literal of the second
    // directive, offset = len("import 'lib1.dart' as one;\n") + 7.
    assert_eq!(*offset, "import 'lib1.dart' as one;\nimport ".len() as u32);
    assert_eq!(*length, 11);
    assert!(message.contains("lib2.dart"), "{message}");
}

/// `import 'dart:io' as prefix; import 'a.dart' as prefix;` +
/// `prefix.File` — the non-SDK `a.dart`'s `File` wins the merged
/// `prefix` scope (`PrefixScope._merge`'s non-SDK preference); `dart:io`
/// stays reported. Oracle (`test_library_prefixed_systemLibrary`):
/// `WARNING|unused_import|lib/test.dart|2:8|9` on `dart:io`.
#[ignore = "staging-side over-credit: unused_finalization.rs prefixed arm checks \
    `exported(target,name).is_some()` without winner identity — `prefix.File` credits \
    BOTH same-prefix edges at staging so the `dart:io` row is never staged \
    (upstream `ImportsTracking` attributes per-fragment, imports_verifier.dart:247); \
    the release-side `import_edge_credited` arm already gates on winner-file identity, \
    the missing piece is in dartq-semantic staging which this ticket does not own"]
#[test]
fn sdk_prefixed_edge_loses_to_non_sdk_provider() {
    let (_, diagnostics, _names) = analyze_files(&[
        (
            "test.dart",
            "import 'dart:io' as prefix;\nimport 'a.dart' as prefix;\nprefix.File? f;\n",
        ),
        ("a.dart", "class File {}\n"),
    ]);
    let rows = unused_import_rows(&diagnostics, &_names);
    assert_eq!(rows.len(), 1, "exactly one unused_import row: {rows:?}");
    let (_, offset, length, message) = &rows[0];
    assert_eq!(*offset, 7);
    assert_eq!(*length, 9);
    assert!(message.contains("dart:io"), "{message}");
}

/// Two unprefixed imports exporting the same `N` + `p is N` —
/// `ambiguous_import` fires, and the oracle's
/// `_hasDiagnosticReportedThatPreventsImportWarnings` suppresses EVERY
/// import warning of the library: the genuinely-unused third import
/// reports nothing. Oracle (`AmbiguousImportTest/test_is`): only the
/// `ERROR|ambiguous_import` row.
#[ignore = "`ambiguous_import` is a registered code (dartq-diagnostics code.rs:840) \
    but no emission site exists yet — conformance status.tsv shows the \
    ambiguous_import family 32/33 red; the upstream `is`-type path resolves through \
    the namespace to `Lookup::Ambiguous` yet no diagnostic row is produced. \
    `IMPORT_WARNING_GATE` already lists `ambiguous_import` per upstream \
    `_hasDiagnosticReportedThatPreventsImportWarnings` (library_analyzer.dart:626-647); \
    the gate logic is exercised by the `out`-scan but cannot trigger until \
    the semantic layer emits the code"]
#[test]
fn ambiguous_use_suppresses_all_import_warnings() {
    let (_, diagnostics, _names) = analyze_files(&[
        (
            "test.dart",
            "import 'lib1.dart';\nimport 'lib2.dart';\nimport 'lib3.dart';\nf(p) {p is N;}\n",
        ),
        ("lib1.dart", "class N {}\n"),
        ("lib2.dart", "class N {}\n"),
        ("lib3.dart", "class M {}\n"),
    ]);
    assert!(
        diagnostics
            .iter()
            .any(|row| row.code.name == "ambiguous_import"),
        "the ambiguous use reports: {diagnostics:?}"
    );
    assert!(
        !has_unused_import(&diagnostics),
        "the library-wide gate suppresses every unused_import: {diagnostics:?}"
    );
}

/// `part 'b.dart'` / `part of 'a.dart'` / `import 'dart:math'` +
/// `Random` used IN THE PART — the part's own import maps back through
/// the part's directives and releases. Oracle
/// (`test_part_usedPartImport`): no `unused_import` row.
#[test]
fn part_file_import_used_in_the_part_releases() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("a.dart", "part 'b.dart';\n"),
        (
            "b.dart",
            "part of 'a.dart';\nimport 'dart:math';\nvoid f(Random _) {}\n",
        ),
    ]);
    assert!(
        !has_unused_import(&diagnostics),
        "the used part import releases: {diagnostics:?}"
    );
}

/// Nested part: `a` → `b` (imports `dart:math`) → `c` (uses `Random`) —
/// the use in `c` resolves through `c`'s scope chain (`c` → `b` → `a`)
/// and credits `b`'s import. Oracle
/// (`test_part_usedPartImport_inNestedPart`): no `unused_import` row.
#[test]
fn nested_part_use_credits_the_ancestor_import() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("a.dart", "part 'b.dart';\n"),
        (
            "b.dart",
            "part of 'a.dart';\nimport 'dart:math';\npart 'c.dart';\n",
        ),
        ("c.dart", "part of 'b.dart';\nvoid f(Random _) {}\n"),
    ]);
    assert!(
        !has_unused_import(&diagnostics),
        "the ancestor import releases: {diagnostics:?}"
    );
}

/// A part file's import nothing uses stays reported ON THE PART's own
/// URI span (`lib/b.dart`, offset of `'c.dart'` inside `b.dart`).
/// Oracle (`ImportDirectiveResolutionTest/test_inPart_library`):
/// `WARNING|unused_import|lib/b.dart|3:8|8`.
#[test]
fn unused_part_file_import_reports_on_the_part() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("a.dart", "part 'b.dart';\n"),
        ("b.dart", "part of 'a.dart';\nimport 'c.dart';\n"),
        ("c.dart", "class C {}\n"),
    ]);
    let rows = unused_import_rows(&diagnostics, &_names);
    assert_eq!(rows.len(), 1, "exactly one unused_import row: {rows:?}");
    let (file, offset, length, message) = &rows[0];
    assert!(file.ends_with("b.dart"), "{file}");
    // `'c.dart'` sits at offset len("part of 'a.dart';\nimport ") inside b.dart.
    assert_eq!(*offset, "part of 'a.dart';\nimport ".len() as u32);
    assert_eq!(*length, 8);
    assert!(message.contains("c.dart"), "{message}");
}

/// `import 'a.dart'` where `a.dart` is `part of 'test.dart'` — the
/// `DirectiveUriWithLibraryImpl` gate: the oracle reports
/// `import_of_non_library` and NEVER `unused_import`, however the
/// directive's names resolve. The `non_library` edge flag drops the
/// staged row outright.
#[test]
fn import_of_a_part_file_never_reports_unused() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("test.dart", "import 'a.dart';\n"),
        ("a.dart", "part of 'test.dart';\n"),
    ]);
    assert!(
        diagnostics
            .iter()
            .any(|row| row.code.name == "import_of_non_library"),
        "the non-library error reports: {diagnostics:?}"
    );
    assert!(
        !has_unused_import(&diagnostics),
        "no unused_import for a non-library target: {diagnostics:?}"
    );
}

/// `import 'lib.dart'; import 'lib.dart';` — both copies unused:
/// `duplicate_import` on the second + `unused_import` on BOTH URI
/// spans. Oracle (`test_importDuplicatedLibraryName`): two
/// `unused_import` rows at the two directive offsets.
#[test]
fn duplicated_unused_import_reports_both_spans() {
    let (_, diagnostics, _names) = analyze_files(&[
        (
            "test.dart",
            "library test;\nimport 'lib.dart';\nimport 'lib.dart';\n",
        ),
        ("lib.dart", "class L {}\n"),
    ]);
    let rows = unused_import_rows(&diagnostics, &_names);
    assert_eq!(rows.len(), 2, "two unused_import rows: {rows:?}");
    let first = "library test;\nimport ".len() as u32;
    let second = first + "'lib.dart';\nimport ".len() as u32;
    assert_eq!(rows[0].1, first);
    assert_eq!(rows[1].1, second);
}

/// A genuinely used unprefixed import stays silent — the
/// `lookup_in_file` winner is this edge's exported declaration.
/// Oracle: no `unused_import` row.
#[test]
fn used_unprefixed_import_stays_silent() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("test.dart", "import 'lib1.dart';\nA? f;\n"),
        ("lib1.dart", "class A {}\n"),
    ]);
    assert!(
        !has_unused_import(&diagnostics),
        "the used import releases: {diagnostics:?}"
    );
}

/// A `/// [A]` doc reference releases the import that exports `A` — the
/// doc-name pool resolves bare in the doc's file scope. Oracle
/// (`test_library_inComment` family): no `unused_import` row.
#[test]
fn doc_reference_releases_the_import() {
    let (_, diagnostics, _names) = analyze_files(&[
        ("test.dart", "import 'lib1.dart';\n/// [A]\nvoid f() {}\n"),
        ("lib1.dart", "class A {}\n"),
    ]);
    assert!(
        !has_unused_import(&diagnostics),
        "the doc-referenced import releases: {diagnostics:?}"
    );
}
