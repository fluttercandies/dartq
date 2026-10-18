//! Pub universe separation pin (#1344): each analysis context — each package
//! root with its own `.dart_tool/package_config.json` — resolves `package:`
//! imports against its OWN package universe, never the workspace union.
//!
//! Upstream, the context locator splits the analyzed tree at every
//! locally-different packages file (upstream Dart SDK source,
//! pkg/analyzer/lib/src/context/context_locator.dart `_createContextRoots`),
//! so a name visible only in a sibling's universe stays undefined here, and
//! a name only this context's config lists resolves — the rust-analyzer
//! crate-graph shape (upstream rust-analyzer source: per-crate dependency
//! closure, not a global symbol table). The fixture mirrors the assembly the
//! CLI ships: the on-disk root config lists only the root package, the
//! nested-root extension joins the `example` entry in memory, and the
//! identity table builds over the extended config — the exact #1340 shape
//! whose flat join this separation fixes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dartq_diagnostics::Diagnostic;
use dartq_engine::{Engine, EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The `pub-nested-example` shape: root package `fixture` (its on-disk
/// config lists only itself) with a nested self-resolved `example/` carrying
/// its own pubspec and its own config listing only itself. The engine
/// assembles over the EXTENDED config — the nested entry joined in memory,
/// the identity table built over it — exactly what the CLI assembly ships.
/// Returns the engine and the workspace root.
fn nested_engine(tag: &str) -> (Engine, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "dartq-engine-pub-universe-{tag}-{}-{}",
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
        "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
    );
    write(
        "pubspec.lock",
        "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
    );
    // The as-written root config: only the root package. `example` joins
    // through the extension, never this file. `packageUri: "lib/"` is
    // load-bearing: pub's writer always emits it (entrypoint.dart:511),
    // and an omitted field is NOT defaulted to `lib/` by the reader —
    // `packageRoot = root` (package_config_json.dart:150-152, the
    // `package_config` parser the analyzer consumes through
    // `parsePackageConfigJsonFile`). Probed on 3.13.1: a config without
    // the field leaves `package:<name>/<file>` landing on the package
    // root, so files under `lib/` do not resolve and every import in this
    // fixture reports `uri_does_not_exist`.
    write(
        ".dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}], "generator": "pub"}"#,
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
    // The nested self-resolved root: own pubspec, own config listing only
    // itself — the sibling universe.
    write(
        "example/pubspec.yaml",
        "name: example\nversion: 0.1.0\nenvironment:\n  sdk: ^3.2.0\n",
    );
    write(
        "example/.dart_tool/package_config.json",
        r#"{"configVersion": 2, "packages": [{"name": "example", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.2"}], "generator": "pub"}"#,
    );
    write("example/lib/greet.dart", "void hi() {}\n");
    write("lib/helper.dart", "void helper() {}\n");

    // Canonicalize before the pub faces resolve it (the #287 spelling
    // universe — macOS /var vs /private/var).
    let root = root.canonicalize().expect("fixture root resolves");
    let config_dir = root.join(".dart_tool");
    // The extended config: the as-written entry plus the nested root — the
    // identity the extension gives the nested package (#1340), which the
    // resolution universe must NOT let leak into the root context.
    let extended = PackageConfig::parse(
        r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}, {"name": "example", "rootUri": "../example", "packageUri": "lib/", "languageVersion": "3.2"}], "generator": "pub"}"#,
    )
    .expect("valid extended config");
    let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("lock");
    let pubspec = Pubspec::parse("name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
        .expect("pubspec");
    let graph = build_graph(
        &extended,
        &lock,
        &pubspec,
        &std::collections::BTreeMap::new(),
        None,
        &config_dir,
    )
    .expect("graph");
    let packages = PackageTable::build(&extended, &graph, &config_dir).expect("table");
    let engine = Engine::new(EngineConfig {
        store: memory_store(),
        sdk: SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads"),
        packages,
        pub_config: Arc::new(extended),
        config_dir,
        semdiag: Some(dartq_engine::semdiag::diagnoser()),
        target: dartq_engine::DEFAULT_TARGET.to_owned(),
        declared_variables: rustc_hash::FxHashMap::default(),
        experiments: Vec::new(),
    })
    .expect("engine");
    (engine, root)
}

/// Whether `code` was reported for the file whose path ends with `tail`.
fn reported(diagnostics: &[Diagnostic], engine: &Engine, tail: &str, code: &str) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code.name == code
            && engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
    })
}

/// The nested boundary, end to end through the real engine and the default
/// diagnoser: a root file importing the sibling's package reports
/// `uri_does_not_exist` on the import and `undefined_function` on the use —
/// the oracle's faces for a name visible only in the sibling's universe.
/// The nested files resolve their OWN universe: importing themselves is
/// silent (where the flat union happened to agree) and importing the ROOT
/// package fails with the same two faces (where the flat union falsely
/// resolved). Each assertion names its file tail, so attribution is pinned
/// alongside the codes.
#[test]
fn nested_context_boundary_matches_the_oracle_split() {
    let (mut engine, root) = nested_engine("boundary");
    let root_main = "import 'package:example/greet.dart';\n\nvoid main() {\n  hi();\n}\n";
    let example_main = "import 'package:example/greet.dart';\n\nvoid main() {\n  hi();\n}\n";
    let example_other = "import 'package:fixture/helper.dart';\n\nvoid tool() {\n  helper();\n}\n";
    // The analyzed world is caller-fed: the CLI's `discover_workspace`
    // enqueues every package root's files before the overlays, and
    // resolution consults the disk only for target EXISTENCE — content
    // enters through the same intake. Without the feed the import targets
    // resolve (their universe membership is what this pin exercises) but
    // stay unresident, which the diagnose-time staleness face codes as
    // `uri_does_not_exist` — the under-feed's own row, not a universe cut.
    engine.set_content(&root.join("example/lib/greet.dart"), b"void hi() {}\n");
    engine.set_content(&root.join("lib/helper.dart"), b"void helper() {}\n");
    engine.set_content(&root.join("lib/main.dart"), root_main.as_bytes());
    engine.set_content(&root.join("example/lib/main.dart"), example_main.as_bytes());
    engine.set_content(
        &root.join("example/lib/other.dart"),
        example_other.as_bytes(),
    );
    let diagnostics = engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");

    // Root context: the sibling package is outside its universe.
    assert!(
        reported(&diagnostics, &engine, "lib/main.dart", "uri_does_not_exist"),
        "the root file's import of the sibling package must not resolve: {diagnostics:?}"
    );
    assert!(
        reported(&diagnostics, &engine, "lib/main.dart", "undefined_function"),
        "the sibling-only name must be undefined in the root context"
    );

    // Nested context: its own universe resolves — silent, as the oracle's
    // example context resolves its own closure.
    assert!(
        !reported(
            &diagnostics,
            &engine,
            "example/lib/main.dart",
            "uri_does_not_exist"
        ),
        "the nested file's import of its own package must resolve: {diagnostics:?}"
    );
    assert!(
        !reported(
            &diagnostics,
            &engine,
            "example/lib/main.dart",
            "undefined_function"
        ),
        "the nested context's own name must be defined"
    );

    // Nested context: the root package is outside ITS universe.
    assert!(
        reported(
            &diagnostics,
            &engine,
            "example/lib/other.dart",
            "uri_does_not_exist"
        ),
        "the nested file's import of the root package must not resolve: {diagnostics:?}"
    );
    assert!(
        reported(
            &diagnostics,
            &engine,
            "example/lib/other.dart",
            "undefined_function"
        ),
        "the root-only name must be undefined in the nested context"
    );
}

/// The flat-union regression: a name in the ROOT context's own universe —
/// the root package itself, listed by the root config as written — still
/// resolves from root files. The separation removes only the extension-
/// joined entries, never the written ones.
#[test]
fn root_files_still_resolve_their_own_universe() {
    let (mut engine, root) = nested_engine("own-universe");
    let tool = "import 'package:fixture/helper.dart';\n\nvoid tool() {\n  helper();\n}\n";
    // The import target joins the analyzed world, exactly as the CLI's
    // `discover_workspace` feeds it before the overlay (see the boundary
    // test's note) — residency, not disk, is the content face.
    engine.set_content(&root.join("lib/helper.dart"), b"void helper() {}\n");
    engine.set_content(&root.join("lib/tool.dart"), tool.as_bytes());
    let diagnostics = engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");
    assert!(
        !reported(&diagnostics, &engine, "lib/tool.dart", "uri_does_not_exist"),
        "the root package must resolve from a root file: {diagnostics:?}"
    );
    assert!(
        !reported(&diagnostics, &engine, "lib/tool.dart", "undefined_function"),
        "the root universe's names must be defined"
    );
}
